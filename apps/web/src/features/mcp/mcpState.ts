import { Api } from '@blob-1/client/api'
import { Effect } from 'effect'
import { Atom } from 'effect/reactivity'

import { AppRuntime } from '~/lib/runtime'

/** Loads and caches the MCP server list across tab switches. */
export const mcpPageAtom = AppRuntime.atom(
  Effect.flatMap(Api, api => api.request('mcp.list', {})),
).pipe(Atom.keepAlive)
