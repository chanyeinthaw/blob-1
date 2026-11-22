import { Option } from 'effect'
import { AsyncResult } from 'effect/reactivity'
import { useEffect, useRef } from 'react'

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
