import { request } from '@blob-1/client/api'
import { Effect } from 'effect'
import { Atom } from 'effect/reactivity'

import { AppRuntime } from '~/lib/runtime'

/** Loads and caches bot-wide General settings and identity across tab switches. */
export const generalPageAtom = AppRuntime.atom(
  Effect.all(
    {
      identity: request('identity.get', {}),
      general: request('general.get', {}),
    },
    { concurrency: 'unbounded' },
  ),
).pipe(Atom.keepAlive)
