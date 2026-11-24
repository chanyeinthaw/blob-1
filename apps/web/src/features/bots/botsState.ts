import { request } from '@blob-1/client/api'
import { Atom } from 'effect/reactivity'

import { AppRuntime, botScopeFromInstanceKey } from '~/lib/runtime'

/** Loads and caches the list of configured bots and their live statuses. */
export const botsListAtom = AppRuntime.atom(request('bots.list', {})).pipe(Atom.keepAlive)

/** Loads and caches per-bot MCP server access toggles keyed by immutable bot instance key. */
export const botMcpAtomFor = Atom.family((instanceKey: string) =>
  AppRuntime.atom(request('bot.mcp.get', botScopeFromInstanceKey(instanceKey))).pipe(
    Atom.keepAlive,
  ),
)
