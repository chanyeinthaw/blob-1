import { request } from '@blob-1/client/api'

import { AppRuntime } from '~/lib/runtime'

/** Loads the model catalog each time the page mounts. */
export const modelsPageAtom = AppRuntime.atom(request('models.get', {}))
