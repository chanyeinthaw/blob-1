import { Api } from '@blob-1/client/api'
import { Effect } from 'effect'
import { Atom } from 'effect/reactivity'

import { AppRuntime, botScopeFromInstanceKey } from '~/lib/runtime'

/** Loads and caches a bot's Discord settings, directory, and general settings. */
export const discordPageAtomFor = Atom.family((instanceKey: string) => {
  const scope = botScopeFromInstanceKey(instanceKey)
  return AppRuntime.atom(
    Effect.flatMap(Api, api =>
      Effect.all(
        {
          discord: api.request('discord.get', scope),
          directory: api.request('discord.directory', scope),
          general: api.request('general.get', scope),
        },
        { concurrency: 'unbounded' },
      ),
    ),
  ).pipe(Atom.keepAlive)
})
