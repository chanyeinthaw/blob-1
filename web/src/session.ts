import { Effect, Schema } from 'effect'

export class WrongPassword extends Schema.TaggedError<WrongPassword>()(
  'WrongPassword',
  { message: Schema.String },
) {}

export class SessionError extends Schema.TaggedError<SessionError>()(
  'SessionError',
  {},
) {}

export const isSignedIn: Effect.Effect<boolean> =
  import.meta.env.VITE_MOCK === '1'
    ? Effect.sync(() => sessionStorage.getItem('blob1.mockSession') === '1')
    : Effect.tryPromise({
        try: signal => fetch('/api/session', { signal }),
        catch: () => new SessionError(),
      }).pipe(
        Effect.map(response => response.status === 204),
        Effect.catch(() => Effect.succeed(false)),
      )

export const signIn = (password: string) =>
  Effect.gen(function* () {
    if (import.meta.env.VITE_MOCK === '1') {
      if (password !== 'blob') {
        return yield* Effect.fail(
          new WrongPassword({ message: 'Wrong password. Try again.' }),
        )
      }
      sessionStorage.setItem('blob1.mockSession', '1')
      return
    }

    const response = yield* Effect.tryPromise({
      try: signal =>
        fetch('/api/login', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ password }),
          signal,
        }),
      catch: () => new SessionError(),
    })
    if (response.status === 401) {
      const body = yield* Effect.tryPromise({
        try: () => response.json(),
        catch: () => new SessionError(),
      }).pipe(Effect.catch(() => Effect.succeed(undefined)))
      const decoded = Schema.decodeUnknownOption(
        Schema.Struct({ error: Schema.String }),
      )(body)
      const message =
        decoded._tag === 'Some' ? decoded.value.error : 'Wrong password'
      return yield* Effect.fail(
        new WrongPassword({
          message:
            message === 'Wrong password'
              ? 'Wrong password. Try again.'
              : message,
        }),
      )
    }
    if (response.status !== 204) {
      return yield* Effect.fail(new SessionError())
    }
  })

export const signOut: Effect.Effect<void, SessionError> = Effect.gen(
  function* () {
    if (import.meta.env.VITE_MOCK === '1') {
      sessionStorage.removeItem('blob1.mockSession')
      return
    }
    const response = yield* Effect.tryPromise({
      try: signal => fetch('/api/logout', { method: 'POST', signal }),
      catch: () => new SessionError(),
    })
    if (response.status !== 204) {
      return yield* Effect.fail(new SessionError())
    }
  },
)
