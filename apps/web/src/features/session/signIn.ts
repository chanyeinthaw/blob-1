import { Session, WrongPassword } from '@blob-1/client/session'
import { Machine } from '@typeonce/effect-machine'
import { Effect, Schema } from 'effect'

const signIn = Effect.fn('SignIn.signIn')(function* (password: string) {
  const session = yield* Session
  return yield* session
    .signIn(password)
    .pipe(
      Effect.catchTag('SessionError', () =>
        Effect.fail(new WrongPassword({ message: 'Could not reach Blob-1. Try again.' })),
      ),
    )
})

export const SignInEvents = Machine.events({
  Submit: { password: Schema.String },
  ClearError: {},
})

export const SignInStates = Machine.state({
  states: {
    Editing: { fields: { error: Schema.NullOr(Schema.String) } },
    Submitting: { fields: { password: Schema.String } },
    SignedIn: {},
  },
})

/** Checks the password with the server; SignedIn is where the screen leaves for settings. */
export const SignInMachine = Machine.make({
  id: 'SignIn',
  root: SignInStates,
  events: SignInEvents,
  effects: { signIn },
}).handle({
  initial: { target: 'Editing', data: { error: null } },
  states: {
    Editing: {
      on: {
        ClearError: {
          update: 'Editing',
          data: { error: null },
        },
        Submit: {
          target: 'Submitting',
          guard: ({ event }) => event.password !== '',
          data: ({ event }) => ({ password: event.password }),
        },
      },
    },
    Submitting: {
      invoke: {
        src: 'signIn',
        input: ({ state }) => state.password,
        onDone: { target: 'SignedIn' },
        onFailure: { target: 'Editing', data: ({ error }) => ({ error: error.message }) },
      },
    },
    SignedIn: {},
  },
})
