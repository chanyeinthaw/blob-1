import { Api } from '@blob-1/client/api'
import { Effect } from 'effect'

import { AppRuntime } from '~/lib/runtime'

/** Loads the Discord settings and directory each time the page mounts. */
export const discordPageAtom = AppRuntime.atom(
  Effect.flatMap(Api, api =>
    Effect.all(
      { discord: api.request('discord.get', {}), directory: api.request('discord.directory', {}) },
      { concurrency: 'unbounded' },
    ),
  ),
)
