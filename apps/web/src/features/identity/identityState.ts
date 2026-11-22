import { request } from '@blob-1/client/api'

import { AppRuntime } from '~/lib/runtime'

/** Loads the saved identity each time the page mounts. */
export const identityPageAtom = AppRuntime.atom(request('identity.get', {}))
