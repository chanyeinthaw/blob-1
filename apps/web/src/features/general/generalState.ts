import { request } from '@blob-1/client/api'
import { Effect } from 'effect'
import { Atom } from 'effect/reactivity'

import { AppRuntime, botScopeFromInstanceKey } from '~/lib/runtime'

/** Loads and caches a bot's General settings and instructions keyed by immutable instance key. */
export const generalPageAtomFor = Atom.family((instanceKey: string) => {
  const scope = botScopeFromInstanceKey(instanceKey)
  return AppRuntime.atom(
    Effect.all(
      {
        instructions: request('instructions.get', scope),
        general: request('general.get', scope),
      },
      { concurrency: 'unbounded' },
    ),
  ).pipe(Atom.keepAlive)
})
