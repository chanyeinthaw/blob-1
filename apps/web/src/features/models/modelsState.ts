import { request } from '@blob-1/client/api'
import { Atom } from 'effect/reactivity'

import { AppRuntime } from '~/lib/runtime'

/** Loads and caches the model catalog across tab switches. */
export const modelsPageAtom = AppRuntime.atom(request('models.get', {})).pipe(Atom.keepAlive)
