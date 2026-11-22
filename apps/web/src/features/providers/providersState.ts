import { Api } from '@blob-1/client/api'
import { Effect } from 'effect'
import { Atom } from 'effect/reactivity'

import { AppRuntime } from '~/lib/runtime'

/** Loads and caches the provider list across tab switches. */
export const providersPageAtom = AppRuntime.atom(
  Effect.flatMap(Api, api => api.request('providers.list', {})),
).pipe(Atom.keepAlive)
