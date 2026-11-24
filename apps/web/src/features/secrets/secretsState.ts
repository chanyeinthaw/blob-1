import { request } from '@blob-1/client/api'
import { Atom } from 'effect/reactivity'

import { AppRuntime, botScopeFromInstanceKey } from '~/lib/runtime'

/** Loads and caches a bot's secret metadata and public URL. */
export const secretsPageAtomFor = Atom.family((instanceKey: string) =>
  AppRuntime.atom(request('secrets.list', botScopeFromInstanceKey(instanceKey))).pipe(
    Atom.keepAlive,
  ),
)
