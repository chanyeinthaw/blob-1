import { Context, Effect, Layer, Option, Schema } from 'effect'

export class WrongPassword extends Schema.TaggedError<WrongPassword>()('WrongPassword', {
  message: Schema.String,
}) {}

export class SessionError extends Schema.TaggedError<SessionError>()('SessionError', {}) {}

export interface SessionService {
  /** Whether the browser holds a live session cookie; fails when the server can't be reached. */
  readonly isSignedIn: Effect.Effect<boolean, SessionError>
  readonly signIn: (password: string) => Effect.Effect<void, WrongPassword | SessionError>
  readonly signOut: Effect.Effect<void, SessionError>
}

export class Session extends Context.Service<Session, SessionService>()('@blob-1/client/session') {}

const LoginFailure = Schema.Struct({ error: Schema.String })
const decodeLoginFailure = Schema.decodeUnknownOption(LoginFailure)

const fetchEffect = (input: string, init: RequestInit = {}) =>
  Effect.tryPromise({
    try: signal => fetch(input, { ...init, signal }),
    catch: () => new SessionError(),
  })

const wrongPassword = (response: Response) =>
  Effect.tryPromise({ try: () => response.json(), catch: () => new SessionError() }).pipe(
    Effect.map(body =>
      Option.match(decodeLoginFailure(body), {
        onNone: () => 'Wrong password. Try again.',
        onSome: ({ error }) => (error === 'Wrong password' ? 'Wrong password. Try again.' : error),
      }),
    ),
    Effect.orElseSucceed(() => 'Wrong password. Try again.'),
    Effect.flatMap(message => Effect.fail(new WrongPassword({ message }))),
  )

/** Cookie sessions against `/api/session`, `/api/login` and `/api/logout`. */
export const sessionLive: Layer.Layer<Session> = Layer.succeed(
  Session,
  Session.of({
    isSignedIn: fetchEffect('/api/session').pipe(
      Effect.flatMap(response => {
        if (response.status === 204) return Effect.succeed(true)
        if (response.status === 401) return Effect.succeed(false)
        return Effect.fail(new SessionError())
      }),
    ),
    signIn: password =>
      fetchEffect('/api/login', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ password }),
      }).pipe(
        Effect.flatMap((response): Effect.Effect<void, WrongPassword | SessionError> => {
          if (response.status === 401) return wrongPassword(response)
          if (response.status !== 204) return Effect.fail(new SessionError())
          return Effect.void
        }),
      ),
    signOut: fetchEffect('/api/logout', { method: 'POST' }).pipe(
      Effect.flatMap(response =>
        response.status === 204 ? Effect.void : Effect.fail(new SessionError()),
      ),
    ),
  }),
)

const MOCK_KEY = 'blob1.mockSession'

/** A session kept in sessionStorage; the password is `blob`. */
export const sessionMock: Layer.Layer<Session> = Layer.succeed(
  Session,
  Session.of({
    isSignedIn: Effect.sync(() => sessionStorage.getItem(MOCK_KEY) === '1'),
    signIn: password =>
      password === 'blob'
        ? Effect.sync(() => sessionStorage.setItem(MOCK_KEY, '1'))
        : Effect.fail(new WrongPassword({ message: 'Wrong password. Try again.' })),
    signOut: Effect.sync(() => sessionStorage.removeItem(MOCK_KEY)),
  }),
)
