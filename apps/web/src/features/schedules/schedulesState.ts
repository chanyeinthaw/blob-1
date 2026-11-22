import { Api } from '@blob-1/client/api'
import { Effect } from 'effect'

import { AppRuntime } from '~/lib/runtime'

/** Loads timezone, scheduled jobs, check-ins, and watches when the page mounts. */
export const schedulesPageAtom = AppRuntime.atom(
  Effect.flatMap(Api, api => api.request('schedule.get', {})),
)
