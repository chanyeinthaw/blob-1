import { request } from '@blob-1/client/api'
import { Atom } from 'effect/reactivity'

import { AppRuntime, botScopeFromInstanceKey } from '~/lib/runtime'

/** Loads and caches a bot's configured webhooks and public URL. */
export const webhooksPageAtomFor = Atom.family((instanceKey: string) =>
  AppRuntime.atom(request('webhooks.list', botScopeFromInstanceKey(instanceKey))).pipe(
    Atom.keepAlive,
  ),
)
