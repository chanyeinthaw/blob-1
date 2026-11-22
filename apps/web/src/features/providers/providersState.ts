import { Api } from '@blob-1/client/api'
import { Effect } from 'effect'

import { AppRuntime } from '~/lib/runtime'

/** Loads the provider list each time the page mounts. */
export const providersPageAtom = AppRuntime.atom(
  Effect.flatMap(Api, api => api.request('providers.list', {})),
)
