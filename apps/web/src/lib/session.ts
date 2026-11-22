import { Session } from '@blob-1/client/session'
import { redirect } from '@tanstack/react-router'
import { Effect } from 'effect'

import { sessionLayer } from './runtime'

const sessionState = Effect.flatMap(Session, session => session.isSignedIn).pipe(
  Effect.map(signedIn => (signedIn ? 'signedIn' : 'signedOut')),
  Effect.orElseSucceed(() => 'unreachable' as const),
  Effect.provide(sessionLayer),
)

/**
 * Route guard: sends a signed-out browser to the sign-in screen. An unreachable server keeps
 * the user in place; the shell shows them offline and reconnects.
 */
export async function requireSession() {
  if ((await Effect.runPromise(sessionState)) === 'signedOut') {
    throw redirect({ to: '/login', replace: true })
  }
}

/** Route guard: skips the sign-in screen for a signed-in browser. */
export async function skipSignInWhenSignedIn() {
  if ((await Effect.runPromise(sessionState)) === 'signedIn') {
    throw redirect({ to: '/settings/general', replace: true })
  }
}
