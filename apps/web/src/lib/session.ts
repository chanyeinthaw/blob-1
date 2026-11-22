import { Session } from '@blob-1/client/session'
import { redirect } from '@tanstack/react-router'
import { Effect } from 'effect'

import { sessionLayer } from './runtime'

let confirmedSignedIn = false

export function markSignedIn() {
  confirmedSignedIn = true
}

export function clearSignedIn() {
  confirmedSignedIn = false
}

const sessionState = Effect.flatMap(Session, session => session.isSignedIn).pipe(
  Effect.map(signedIn => (signedIn ? 'signedIn' : 'signedOut')),
  Effect.orElseSucceed(() => 'unreachable' as const),
  Effect.provide(sessionLayer),
)

/**
 * Route guard: sends a signed-out browser to the sign-in screen. Once confirmed
 * signed in, subsequent settings tab switches return immediately without another
 * `/api/session` round-trip; `UnauthorizedRedirect` clears the cache on expiry.
 */
export async function requireSession() {
  if (confirmedSignedIn) return
  const state = await Effect.runPromise(sessionState)
  if (state === 'signedIn') {
    confirmedSignedIn = true
  } else if (state === 'signedOut') {
    throw redirect({ to: '/login', replace: true })
  }
}

/** Route guard: skips the sign-in screen for a signed-in browser. */
export async function skipSignInWhenSignedIn() {
  if (confirmedSignedIn) {
    throw redirect({ to: '/settings/general', replace: true })
  }
  if ((await Effect.runPromise(sessionState)) === 'signedIn') {
    confirmedSignedIn = true
    throw redirect({ to: '/settings/general', replace: true })
  }
}
