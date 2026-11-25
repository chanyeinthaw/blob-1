import { request } from '@blob-1/client/api'
import { Atom } from 'effect/reactivity'

import { AppRuntime, botScopeFromInstanceKey } from '~/lib/runtime'

/** Loads and caches the newest page of a bot's web chat. */
export const webChatAtomFor = Atom.family((instanceKey: string) =>
  AppRuntime.atom(
    request('chat.history', { ...botScopeFromInstanceKey(instanceKey), before: null }),
  ).pipe(Atom.keepAlive),
)
