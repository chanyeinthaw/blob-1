import { Api } from '@blob-1/client/api'
import { Effect } from 'effect'
import { Atom } from 'effect/reactivity'

import { AppRuntime } from '~/lib/runtime'

/** Loads and caches Discord settings, directory and general settings across tab switches. */
export const discordPageAtom = AppRuntime.atom(
  Effect.flatMap(Api, api =>
    Effect.all(
      {
        discord: api.request('discord.get', {}),
        directory: api.request('discord.directory', {}),
        general: api.request('general.get', {}),
      },
      { concurrency: 'unbounded' },
    ),
  ),
).pipe(Atom.keepAlive)
