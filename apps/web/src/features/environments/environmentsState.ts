import { request } from '@blob-1/client/api'
import { Atom } from 'effect/reactivity'

import { AppRuntime } from '~/lib/runtime'

/** Loads and caches pending nodes and accepted environments across tab switches. */
export const environmentsPageAtom = AppRuntime.atom(request('environments.list', {})).pipe(
  Atom.keepAlive,
)
