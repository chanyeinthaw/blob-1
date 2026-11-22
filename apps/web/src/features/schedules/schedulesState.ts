import { Api } from '@blob-1/client/api'
import { Effect } from 'effect'
import { Atom } from 'effect/reactivity'

import { AppRuntime } from '~/lib/runtime'

/** Loads and caches timezone, scheduled jobs, check-ins, and watches across tab switches. */
export const schedulesPageAtom = AppRuntime.atom(
  Effect.flatMap(Api, api => api.request('schedule.get', {})),
).pipe(Atom.keepAlive)
