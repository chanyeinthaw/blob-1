import { request } from '@blob-1/client/api'
import { Atom } from 'effect/reactivity'

import { AppRuntime, botScopeFromInstanceKey } from '~/lib/runtime'

export type UsageRange = 1 | 7 | 30

export const USAGE_RANGES: ReadonlyArray<{
  readonly days: UsageRange
  readonly label: string
}> = [
  { days: 1, label: '24h' },
  { days: 7, label: '7d' },
  { days: 30, label: '30d' },
]

const usageAtomFamily = Atom.family((key: string) => {
  const sep = key.lastIndexOf(':')
  const scope = botScopeFromInstanceKey(key.slice(0, sep))
  const days = Number(key.slice(sep + 1))
  return AppRuntime.atom(request('usage.get', { ...scope, days })).pipe(Atom.keepAlive)
})

/** Returns the cached keep-alive usage atom for a bot instance and time window in days. */
export const usageAtomFor = (instanceKey: string, days: UsageRange) =>
  usageAtomFamily(`${instanceKey}:${days}`)
