import { request } from '@blob-1/client/api'
import { Atom } from 'effect/reactivity'

import { AppRuntime } from '~/lib/runtime'

/** Loads and caches the configured webhooks and public URL across tab switches. */
export const webhooksPageAtom = AppRuntime.atom(request('webhooks.list', {})).pipe(Atom.keepAlive)
