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

/** Loads and caches the bot's managed and detected skills keyed by immutable bot instance key. */
export const botSkillsAtomFor = Atom.family((instanceKey: string) =>
  AppRuntime.atom(request('bot.skills.list', botScopeFromInstanceKey(instanceKey))).pipe(
    Atom.keepAlive,
  ),
)

/**
 * Loads one skill's full description, `SKILL.md` body and file list. Keyed by
 * `<instanceKey>/<name>`; not kept alive, so each open reads it fresh.
 */
export const botSkillDetailAtomFor = Atom.family((key: string) => {
  const sep = key.lastIndexOf('/')
  return AppRuntime.atom(
    request('bot.skills.read', {
      ...botScopeFromInstanceKey(key.slice(0, sep)),
      name: key.slice(sep + 1),
    }),
  )
})
