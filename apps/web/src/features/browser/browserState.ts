import { request } from '@blob-1/client/api'
import { Atom } from 'effect/reactivity'

import { AppRuntime } from '~/lib/runtime'

/** Loads and caches browser status and profiles across tab switches. */
export const browserPageAtom = AppRuntime.atom(request('browser.status', {})).pipe(Atom.keepAlive)
