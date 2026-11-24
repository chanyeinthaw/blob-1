import { request } from '@blob-1/client/api'
import { Atom } from 'effect/reactivity'

import { AppRuntime, botScopeFromInstanceKey } from '~/lib/runtime'

/** Loads and caches a bot's model assignments and the shared model catalog. */
export const modelsPageAtomFor = Atom.family((instanceKey: string) =>
  AppRuntime.atom(request('models.get', botScopeFromInstanceKey(instanceKey))).pipe(Atom.keepAlive),
)

/** Shared global model catalog atom (reads catalog via the default bot). */
export const modelsCatalogAtom = modelsPageAtomFor('default')
