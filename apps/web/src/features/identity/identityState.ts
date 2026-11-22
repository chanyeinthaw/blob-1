import { request } from '@blob-1/client/api'
import { Effect } from 'effect'

import { AppRuntime } from '~/lib/runtime'

/** Loads bot-wide General settings and identity each time the page mounts. */
export const generalPageAtom = AppRuntime.atom(
  Effect.all({
    identity: request('identity.get', {}),
    general: request('general.get', {}),
  }),
)
