import type { UsageKind, UsageSummary } from '@blob-1/contracts'
import { useAtomRefresh, useAtomValue } from '@effect/atom-react'
import { AsyncResult } from 'effect/reactivity'
import { useState } from 'react'

import { PageError, Section } from '~/components/layout/Page'
import { ToggleGroup, ToggleGroupItem } from '~/components/ui/toggle-group'
import { matchCached, useBackgroundRefresh } from '~/lib/useBackgroundRefresh'

import { USAGE_RANGES, type UsageRange, usageAtomFor } from './usageState'

const KIND_LABELS = {
  chat: 'Chat',
  task: 'Task',
  utility: 'Utility',
  compaction: 'Compaction',
} satisfies Record<UsageKind, string>

function formatTokens(count: number): string {
  if (count >= 1_000_000) {
    return `${(count / 1_000_000).toFixed(2).replace(/\.?0+$/, '')}M`
  }
  if (count >= 1_000) {
    return `${(count / 1_000).toFixed(1).replace(/\.0$/, '')}k`
  }
  return String(count)
}

function formatHitRate(cached: number, input: number): string {
  if (input <= 0) return '0%'
  const pct = Math.min(100, Math.round((cached / input) * 100))
  return `${pct}%`
}

interface StatCellProps {
  readonly label: string
  readonly value: string
}

function StatCell(props: StatCellProps) {
  return (
    <div className="px-4 py-3">
      <p className="text-xs text-muted-foreground">{props.label}</p>
      <p className="mt-0.5 text-base font-semibold tracking-tight text-foreground tabular-nums">
        {props.value}
      </p>
    </div>
  )
}

interface BreakdownRowProps {
  readonly label: string
  readonly mono?: boolean
  readonly calls: number
  readonly inputTokens: number
  readonly cachedTokens: number
  readonly outputTokens: number
}

function BreakdownRow(props: BreakdownRowProps) {
  return (
    <li className="flex flex-wrap items-center justify-between gap-x-4 gap-y-1 px-4 py-2 text-xs">
      <div className="flex min-w-0 items-center gap-2">
        <span
          className={
            props.mono === true
              ? 'truncate font-mono text-foreground'
              : 'font-medium text-foreground'
          }
        >
          {props.label}
        </span>
        <span className="shrink-0 text-muted-foreground tabular-nums">
          {props.calls.toLocaleString()} {props.calls === 1 ? 'call' : 'calls'}
        </span>
      </div>
      <div className="flex shrink-0 items-center gap-3 text-muted-foreground tabular-nums">
        <span>in {formatTokens(props.inputTokens)}</span>
        <span>cached {formatTokens(props.cachedTokens)}</span>
        <span>out {formatTokens(props.outputTokens)}</span>
        <span className="w-9 text-right font-medium text-foreground">
          {formatHitRate(props.cachedTokens, props.inputTokens)}
        </span>
      </div>
    </li>
  )
}

interface UsageSummaryBodyProps {
  readonly summary: UsageSummary
}

function UsageSummaryBody(props: UsageSummaryBodyProps) {
  const { totals, byKind, byModel } = props.summary
  if (totals.calls === 0) {
    return <p className="px-4 py-4 text-sm text-muted-foreground">No usage recorded yet.</p>
  }

  const totalTokens = totals.inputTokens + totals.outputTokens

  return (
    <div className="divide-y divide-border">
      <div className="grid grid-cols-2 divide-x divide-y divide-border sm:grid-cols-4 sm:divide-y-0">
        <StatCell label="Total tokens" value={formatTokens(totalTokens)} />
        <StatCell
          label="Cache hit"
          value={formatHitRate(totals.cachedTokens, totals.inputTokens)}
        />
        <StatCell label="Calls" value={totals.calls.toLocaleString()} />
        <StatCell label="Output" value={formatTokens(totals.outputTokens)} />
      </div>
      {byKind.length === 0 ? null : (
        <div>
          <p className="px-4 pt-2.5 pb-1 text-[11px] font-medium tracking-wide text-muted-foreground uppercase">
            By role
          </p>
          <ul className="divide-y divide-border/60 pb-1">
            {byKind.map(bucket => (
              <BreakdownRow
                key={bucket.kind}
                label={KIND_LABELS[bucket.kind]}
                calls={bucket.calls}
                inputTokens={bucket.inputTokens}
                cachedTokens={bucket.cachedTokens}
                outputTokens={bucket.outputTokens}
              />
            ))}
          </ul>
        </div>
      )}
      {byModel.length === 0 ? null : (
        <div>
          <p className="px-4 pt-2.5 pb-1 text-[11px] font-medium tracking-wide text-muted-foreground uppercase">
            By model
          </p>
          <ul className="divide-y divide-border/60 pb-1">
            {byModel.map(bucket => (
              <BreakdownRow
                key={`${bucket.provider}/${bucket.model}`}
                label={`${bucket.provider}/${bucket.model}`}
                mono
                calls={bucket.calls}
                inputTokens={bucket.inputTokens}
                cachedTokens={bucket.cachedTokens}
                outputTokens={bucket.outputTokens}
              />
            ))}
          </ul>
        </div>
      )}
    </div>
  )
}

interface UsageCardProps {
  readonly instanceKey: string
  readonly days: UsageRange
}

function UsageCard(props: UsageCardProps) {
  const atom = usageAtomFor(props.instanceKey, props.days)
  const result = useAtomValue(atom)
  const refresh = useAtomRefresh(atom)
  useBackgroundRefresh(refresh, AsyncResult.isSuccess(result))

  return matchCached(result, {
    onInitial: () => (
      <div className="overflow-hidden rounded-2xl border border-border bg-card shadow-xs">
        <p className="px-4 py-4 text-sm text-muted-foreground">Loading…</p>
      </div>
    ),
    onFailure: failure => (
      <div className="overflow-hidden rounded-2xl border border-border bg-card px-4 py-3 shadow-xs">
        <PageError cause={failure.cause} onRetry={refresh} />
      </div>
    ),
    onSuccess: summary => (
      <div className="overflow-hidden rounded-2xl border border-border bg-card shadow-xs">
        <UsageSummaryBody summary={summary} />
      </div>
    ),
  })
}

interface UsageSectionProps {
  readonly instanceKey: string
}

/** Compact token and cache usage summary with a 24h / 7d / 30d window toggle. */
export function UsageSection(props: UsageSectionProps) {
  const [days, setDays] = useState<UsageRange>(7)

  return (
    <Section
      title="Usage"
      action={
        <ToggleGroup
          aria-label="Usage window"
          spacing={1}
          value={[String(days)]}
          onValueChange={values => {
            const next = USAGE_RANGES.find(option => String(option.days) === values[0])?.days
            if (next !== undefined) setDays(next)
          }}
          className="h-8 rounded-full bg-muted p-[3px]"
        >
          {USAGE_RANGES.map(option => (
            <ToggleGroupItem
              key={option.days}
              value={String(option.days)}
              className="h-full rounded-full px-2.5 text-xs text-muted-foreground hover:bg-transparent hover:text-foreground aria-pressed:bg-background aria-pressed:text-foreground aria-pressed:shadow-sm"
            >
              {option.label}
            </ToggleGroupItem>
          ))}
        </ToggleGroup>
      }
    >
      <UsageCard key={`${props.instanceKey}:${days}`} instanceKey={props.instanceKey} days={days} />
    </Section>
  )
}
