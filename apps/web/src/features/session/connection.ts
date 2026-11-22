import { Api } from '@blob-1/client/api'
import { Session } from '@blob-1/client/session'
import { Effect, Stream } from 'effect'

import { AppRuntime } from '~/lib/runtime'

/** Whether the socket is open; subscribing opens it, so only the signed-in shell reads this. */
export const connectionAtom = AppRuntime.atom(Stream.unwrap(Effect.map(Api, api => api.status)))

/** Emits once the server rejects the session, e.g. after it expired. */
export const unauthorizedAtom = AppRuntime.atom(
  Stream.unwrap(Effect.map(Api, api => api.unauthorized)),
)

export const signOutAtom = AppRuntime.fn(() => Effect.flatMap(Session, session => session.signOut))
