import { Api } from '@blob-1/client/api'
import { Effect } from 'effect'

import { AppRuntime } from '~/lib/runtime'

/** Loads the MCP server list each time the page mounts. */
export const mcpPageAtom = AppRuntime.atom(Effect.flatMap(Api, api => api.request('mcp.list', {})))
