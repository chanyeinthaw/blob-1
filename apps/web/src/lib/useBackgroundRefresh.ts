import type { ChangeTopic } from '@blob-1/contracts'
import { useAtomValue } from '@effect/atom-react'
import { Option } from 'effect'
import { AsyncResult } from 'effect/reactivity'
import { useEffect, useRef, useState } from 'react'

import { changeCountAtom } from './runtime'

/**
 * Matches an `AsyncResult` while keeping the last successful value rendered if a
 * background refresh fails, showing the full-page error only when no cache exists.
 */
export function matchCached<A, E, R>(
  result: AsyncResult.AsyncResult<A, E>,
  handlers: {
    readonly onInitial: () => R
    readonly onFailure: (failure: AsyncResult.Failure<A, E>) => R
    readonly onSuccess: (value: A) => R
  },
): R {
  return AsyncResult.match(result, {
    onInitial: handlers.onInitial,
    onFailure: failure =>
      Option.match(failure.previousSuccess, {
        onSome: prev => handlers.onSuccess(prev.value),
        onNone: () => handlers.onFailure(failure),
      }),
    onSuccess: success => handlers.onSuccess(success.value),
  })
}

/** Triggers one quiet background refresh on mount once cached data is already present. */
export function useBackgroundRefresh(refresh: () => void, hadCachedValue: boolean) {
  const shouldRefresh = useRef(hadCachedValue)
  useEffect(() => {
    if (shouldRefresh.current) {
      refresh()
    }
  }, [refresh])
}

/**
 * Calls `refresh` whenever the server pushes a change for this bot and topic,
 * or the socket reconnects. For pages whose data lives in a machine; plain
 * data atoms read `changeCountAtom` directly. `refresh` must be stable.
 */
export function useChangeRefresh(instanceKey: string, topic: ChangeTopic, refresh: () => void) {
  const count = AsyncResult.getOrElse(useAtomValue(changeCountAtom(instanceKey, topic)), () => 0)
  // The counter outlives the page; pushes seen before this mount are covered
  // by the page's own mount refresh.
  const [baseline] = useState(count)
  useEffect(() => {
    if (count !== baseline) refresh()
  }, [baseline, count, refresh])
}
