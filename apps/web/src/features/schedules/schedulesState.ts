import { Api } from '@blob-1/client/api'
import { Effect } from 'effect'
import { Atom } from 'effect/reactivity'

import { AppRuntime, botScopeFromInstanceKey } from '~/lib/runtime'

/** Loads and caches a bot's timezone, scheduled jobs, check-ins, and watches. */
export const schedulesPageAtomFor = Atom.family((instanceKey: string) =>
  AppRuntime.atom(
    Effect.flatMap(Api, api => api.request('schedule.get', botScopeFromInstanceKey(instanceKey))),
  ).pipe(Atom.keepAlive),
)
