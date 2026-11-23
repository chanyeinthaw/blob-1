import { request } from '@blob-1/client/api'
import { Atom } from 'effect/reactivity'

import { AppRuntime } from '~/lib/runtime'

export type UsageRange = 1 | 7 | 30

export const USAGE_RANGES: ReadonlyArray<{
  readonly days: UsageRange
  readonly label: string
}> = [
  { days: 1, label: '24h' },
  { days: 7, label: '7d' },
  { days: 30, label: '30d' },
]

const makeUsageAtom = (days: UsageRange) =>
  AppRuntime.atom(request('usage.get', { days })).pipe(Atom.keepAlive)

const usageAtoms = {
  1: makeUsageAtom(1),
  7: makeUsageAtom(7),
  30: makeUsageAtom(30),
}

/** Returns the cached keep-alive usage atom for the given window in days. */
export const usageAtomFor = (days: UsageRange) => usageAtoms[days]
