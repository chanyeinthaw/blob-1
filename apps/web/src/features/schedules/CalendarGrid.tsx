import type { CheckinConfig, ScheduleItem, ScheduleOccurrence } from '@blob-1/contracts'
import { Clock, Repeat } from 'lucide-react'
import { useEffect, useMemo, useRef, useState } from 'react'

import { Dialog, DialogContent, DialogHeader, DialogTitle } from '~/components/ui/dialog'
import { Tooltip, TooltipContent, TooltipTrigger } from '~/components/ui/tooltip'
import { cn } from '~/lib/utils'

import { type DayCell, formatSlotLocal, pad2, toZonedDate } from './calendarMath'

export interface CalendarEvent {
  readonly key: string
  readonly schedule: ScheduleItem
  readonly at: number
  readonly dayKey: string
  readonly hour: number
  readonly minute: number
  readonly timeLabel: string
}

const HOURS_24 = Array.from({ length: 24 }, (_, i) => i)
const HOUR_HEIGHT = 56

function formatIntervalShort(seconds: number): string {
  if (seconds % 86400 === 0) return `${seconds / 86400}d`
  if (seconds % 3600 === 0) return `${seconds / 3600}h`
  return `${Math.round(seconds / 60)}m`
}

function buildEventsByDay(
  schedules: ReadonlyArray<ScheduleItem>,
  occurrences: ReadonlyArray<ScheduleOccurrence>,
  timeZone: string,
): Map<string, Array<CalendarEvent>> {
  const byId = new Map(schedules.map(item => [item.id, item]))
  const grouped = new Map<string, Array<CalendarEvent>>()
  for (const occ of occurrences) {
    const schedule = byId.get(occ.scheduleId)
    if (schedule === undefined) continue
    const zoned = toZonedDate(occ.at, timeZone)
    const event: CalendarEvent = {
      key: `${occ.scheduleId}:${occ.at}`,
      schedule,
      at: occ.at,
      dayKey: zoned.key,
      hour: zoned.hour,
      minute: zoned.minute,
      timeLabel: `${pad2(zoned.hour)}:${pad2(zoned.minute)}`,
    }
    const list = grouped.get(zoned.key)
    if (list === undefined) {
      grouped.set(zoned.key, [event])
    } else {
      list.push(event)
    }
  }
  return grouped
}

interface WeekCalendarProps {
  readonly cells: ReadonlyArray<DayCell>
  readonly schedules: ReadonlyArray<ScheduleItem>
  readonly occurrences: ReadonlyArray<ScheduleOccurrence>
  readonly checkin: CheckinConfig | null
  readonly timezone: string
  readonly nowSeconds: number
  readonly onSelectSlot: (prefillWhen: string) => void
  readonly onSelectEvent: (scheduleId: string) => void
}

interface SlotBucket {
  readonly key: string
  readonly topPx: number
  readonly events: ReadonlyArray<CalendarEvent>
}

const CARD_HEIGHT = 54

function groupDayBuckets(dayEvents: ReadonlyArray<CalendarEvent>): ReadonlyArray<SlotBucket> {
  const maxTop = 24 * HOUR_HEIGHT - CARD_HEIGHT
  const sorted = dayEvents
    .map(ev => ({
      ev,
      topPx: Math.min(((ev.hour * 60 + ev.minute) / 60) * HOUR_HEIGHT, maxTop),
    }))
    .toSorted((a, b) => a.topPx - b.topPx || a.ev.at - b.ev.at)
  const groups: Array<{ readonly topPx: number; readonly events: Array<CalendarEvent> }> = []
  for (const { ev, topPx } of sorted) {
    const last = groups.at(-1)
    if (last !== undefined && topPx - last.topPx < CARD_HEIGHT) {
      last.events.push(ev)
    } else {
      groups.push({ topPx, events: [ev] })
    }
  }
  return groups.map(group => ({
    key: group.events[0]?.key ?? String(group.topPx),
    topPx: group.topPx,
    events: group.events,
  }))
}

/** Full edge-to-edge 7-day timeline grid with shaded check-in windows and plotted jobs. */
export function WeekCalendar(props: WeekCalendarProps) {
  const scrollRef = useRef<HTMLDivElement>(null)
  const [overflowBucket, setOverflowBucket] = useState<{
    readonly title: string
    readonly events: ReadonlyArray<CalendarEvent>
  } | null>(null)
  const eventsByDay = useMemo(
    () => buildEventsByDay(props.schedules, props.occurrences, props.timezone),
    [props.schedules, props.occurrences, props.timezone],
  )
  const nowZoned = toZonedDate(props.nowSeconds, props.timezone)
  const nowMinutes = nowZoned.hour * 60 + nowZoned.minute

  useEffect(() => {
    const el = scrollRef.current
    if (el !== null) {
      el.scrollTop = 7 * HOUR_HEIGHT
    }
  }, [])

  return (
    <div className="flex min-h-0 flex-1 flex-col overflow-hidden">
      <div className="grid shrink-0 grid-cols-[52px_repeat(7,minmax(0,1fr))] border-b border-border/80 bg-background">
        <div className="border-r border-border/60" />
        {props.cells.map(cell => {
          const checkinActive = props.checkin !== null && props.checkin.days.includes(cell.weekday)
          return (
            <div
              key={cell.key}
              className="flex flex-col items-center justify-center gap-0.5 border-r border-border/60 py-2 last:border-r-0"
            >
              <span className="flex items-center gap-1 text-[11px] font-medium tracking-wide text-muted-foreground uppercase">
                {cell.weekdayShort}
                {checkinActive && props.checkin !== null ? (
                  <Tooltip>
                    <TooltipTrigger
                      render={
                        <span
                          aria-label={`Check-ins every ${formatIntervalShort(props.checkin.every)}, ${pad2(props.checkin.startHour)}:00 to ${pad2(props.checkin.endHour)}:00`}
                          className="size-1.5 rounded-full bg-ok"
                        />
                      }
                    />
                    <TooltipContent>
                      Check-ins · {formatIntervalShort(props.checkin.every)} (
                      {pad2(props.checkin.startHour)}:00–
                      {pad2(props.checkin.endHour)}:00)
                    </TooltipContent>
                  </Tooltip>
                ) : null}
              </span>
              <span
                className={cn(
                  'grid size-7 place-items-center rounded-full text-sm font-semibold',
                  cell.isToday ? 'bg-primary text-primary-foreground' : 'text-foreground',
                )}
              >
                {cell.day}
              </span>
            </div>
          )
        })}
      </div>
      <div ref={scrollRef} className="min-h-0 flex-1 overflow-y-auto">
        <div
          className="relative grid grid-cols-[52px_repeat(7,minmax(0,1fr))]"
          style={{ height: `${24 * HOUR_HEIGHT}px` }}
        >
          <div className="relative border-r border-border/60 bg-background">
            {HOURS_24.map(hour => (
              <div
                key={hour}
                className="relative pr-2 text-right"
                style={{ height: `${HOUR_HEIGHT}px` }}
              >
                {hour === 0 ? null : (
                  <span className="absolute -top-2 right-2 font-mono text-[10px] text-muted-foreground">
                    {pad2(hour)}:00
                  </span>
                )}
              </div>
            ))}
          </div>
          {props.cells.map(cell => {
            const dayEvents = eventsByDay.get(cell.key) ?? []
            const checkinActive =
              props.checkin !== null && props.checkin.days.includes(cell.weekday)
            return (
              <div key={cell.key} className="relative border-r border-border/60 last:border-r-0">
                {HOURS_24.map(hour => (
                  <button
                    key={hour}
                    type="button"
                    aria-label={`Schedule on ${cell.key} at ${pad2(hour)}:00`}
                    onClick={() => props.onSelectSlot(formatSlotLocal(cell, hour))}
                    className="block w-full border-b border-border/40 text-left transition-colors hover:bg-muted/40"
                    style={{ height: `${HOUR_HEIGHT}px` }}
                  />
                ))}
                {checkinActive && props.checkin !== null ? (
                  <div
                    aria-hidden="true"
                    className="pointer-events-none absolute inset-x-0 bg-ok/[0.08]"
                    style={{
                      top: `${props.checkin.startHour * HOUR_HEIGHT}px`,
                      height: `${(props.checkin.endHour - props.checkin.startHour) * HOUR_HEIGHT}px`,
                    }}
                  />
                ) : null}
                {cell.isToday ? (
                  <div
                    aria-hidden="true"
                    className="pointer-events-none absolute inset-x-0 z-10 flex items-center"
                    style={{ top: `${(nowMinutes / 60) * HOUR_HEIGHT}px` }}
                  >
                    <span className="-ml-1 size-2 rounded-full bg-destructive" />
                    <span className="h-[1.5px] flex-1 bg-destructive" />
                  </div>
                ) : null}
                {groupDayBuckets(dayEvents).map(bucket => {
                  const primary = bucket.events[0]
                  if (primary === undefined) return null
                  const extraCount = bucket.events.length - 1
                  return (
                    <div
                      key={bucket.key}
                      style={{ top: `${bucket.topPx}px` }}
                      className="absolute inset-x-0.5 z-20 flex flex-col gap-0.5 sm:inset-x-1"
                    >
                      <button
                        type="button"
                        onClick={event => {
                          event.stopPropagation()
                          if (extraCount > 0) {
                            setOverflowBucket({
                              title: `${cell.key} · ${primary.timeLabel}`,
                              events: bucket.events,
                            })
                          } else {
                            props.onSelectEvent(primary.schedule.id)
                          }
                        }}
                        style={{ height: `${CARD_HEIGHT}px` }}
                        className={cn(
                          'flex w-full flex-col overflow-hidden rounded-lg border px-1.5 py-1 text-left shadow-xs transition-colors sm:px-2',
                          primary.schedule.kind === 'cron'
                            ? 'border-primary/25 bg-[color-mix(in_oklch,var(--background)_92%,var(--primary))] text-foreground hover:bg-[color-mix(in_oklch,var(--background)_87%,var(--primary))]'
                            : 'border-border bg-background text-foreground hover:bg-muted',
                        )}
                      >
                        <span className="flex min-w-0 items-center justify-between gap-1 font-mono text-[10px] font-medium text-muted-foreground">
                          <span className="flex min-w-0 items-center gap-1 truncate">
                            {primary.schedule.kind === 'cron' ? (
                              <Repeat className="hidden size-2.5 shrink-0 sm:inline-block" />
                            ) : (
                              <Clock className="hidden size-2.5 shrink-0 sm:inline-block" />
                            )}
                            <span className="truncate">{primary.timeLabel}</span>
                          </span>
                          {extraCount > 0 ? (
                            <span className="shrink-0 rounded-full bg-primary/15 px-1 text-[9px] font-semibold text-foreground">
                              +{extraCount}
                            </span>
                          ) : null}
                        </span>
                        <span className="line-clamp-2 w-full text-[11px] leading-tight font-medium break-words [overflow-wrap:anywhere] sm:text-xs">
                          {primary.schedule.text}
                        </span>
                      </button>
                    </div>
                  )
                })}
              </div>
            )
          })}
        </div>
      </div>
      <Dialog
        open={overflowBucket !== null}
        onOpenChange={open => {
          if (!open) setOverflowBucket(null)
        }}
      >
        <DialogContent className="sm:max-w-sm">
          <DialogHeader>
            <DialogTitle>{overflowBucket?.title ?? ''}</DialogTitle>
          </DialogHeader>
          <ul className="flex max-h-80 flex-col divide-y divide-border/60 overflow-y-auto">
            {(overflowBucket?.events ?? []).map(ev => (
              <li key={ev.key}>
                <button
                  type="button"
                  onClick={() => {
                    setOverflowBucket(null)
                    props.onSelectEvent(ev.schedule.id)
                  }}
                  className="flex w-full items-start gap-2.5 rounded-lg px-2 py-2 text-left transition-colors hover:bg-muted"
                >
                  <span className="mt-0.5 shrink-0 font-mono text-xs text-muted-foreground">
                    {ev.timeLabel}
                  </span>
                  <span className="min-w-0 flex-1 break-words text-sm [overflow-wrap:anywhere]">
                    {ev.schedule.text}
                  </span>
                </button>
              </li>
            ))}
          </ul>
        </DialogContent>
      </Dialog>
    </div>
  )
}

interface MonthCalendarProps {
  readonly cells: ReadonlyArray<DayCell>
  readonly schedules: ReadonlyArray<ScheduleItem>
  readonly occurrences: ReadonlyArray<ScheduleOccurrence>
  readonly checkin: CheckinConfig | null
  readonly timezone: string
  readonly onSelectSlot: (prefillWhen: string) => void
  readonly onSelectEvent: (scheduleId: string) => void
}

const MONTH_HEADERS = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat']

/** Full edge-to-edge month grid showing daily jobs and check-in day markers. */
export function MonthCalendar(props: MonthCalendarProps) {
  const [overflowDay, setOverflowDay] = useState<string | null>(null)
  const eventsByDay = useMemo(
    () => buildEventsByDay(props.schedules, props.occurrences, props.timezone),
    [props.schedules, props.occurrences, props.timezone],
  )
  const weeks = props.cells.length / 7
  const overflowEvents = overflowDay === null ? [] : (eventsByDay.get(overflowDay) ?? [])

  return (
    <div className="flex min-h-0 flex-1 flex-col overflow-hidden">
      <div className="grid shrink-0 grid-cols-7 border-b border-border/80 bg-background">
        {MONTH_HEADERS.map(label => (
          <div
            key={label}
            className="border-r border-border/60 py-2 text-center text-[11px] font-medium tracking-wide text-muted-foreground uppercase last:border-r-0"
          >
            {label}
          </div>
        ))}
      </div>
      <div
        className="grid min-h-0 flex-1 grid-cols-7 overflow-y-auto"
        style={{ gridTemplateRows: `repeat(${weeks}, minmax(64px, 1fr))` }}
      >
        {props.cells.map(cell => {
          const dayEvents = eventsByDay.get(cell.key) ?? []
          const checkinActive = props.checkin !== null && props.checkin.days.includes(cell.weekday)
          return (
            <div
              key={cell.key}
              className={cn(
                'flex min-w-0 flex-col border-r border-b border-border/60 p-1.5 last:border-r-0',
                !cell.inMonth && 'bg-muted/25 text-muted-foreground',
              )}
            >
              <div className="mb-1 flex items-center justify-between">
                <button
                  type="button"
                  onClick={() => props.onSelectSlot(formatSlotLocal(cell, 9))}
                  className={cn(
                    'grid size-6 place-items-center rounded-full text-xs font-medium transition-colors',
                    cell.isToday ? 'bg-primary text-primary-foreground' : 'hover:bg-muted',
                  )}
                >
                  {cell.day}
                </button>
                {checkinActive && props.checkin !== null ? (
                  <Tooltip>
                    <TooltipTrigger
                      render={
                        <span
                          aria-label={`Check-ins ${pad2(props.checkin.startHour)}:00 to ${pad2(props.checkin.endHour)}:00`}
                          className="size-1.5 rounded-full bg-ok"
                        />
                      }
                    />
                    <TooltipContent>
                      Check-ins {pad2(props.checkin.startHour)}:00–{pad2(props.checkin.endHour)}:00
                    </TooltipContent>
                  </Tooltip>
                ) : null}
              </div>
              <div className="flex min-w-0 flex-col gap-1">
                {dayEvents.slice(0, 3).map(ev => (
                  <button
                    key={ev.key}
                    type="button"
                    onClick={() => props.onSelectEvent(ev.schedule.id)}
                    className={cn(
                      'flex w-full items-center gap-1 truncate rounded-md px-1.5 py-0.5 text-left text-[11px] font-medium transition-colors',
                      ev.schedule.kind === 'cron'
                        ? 'bg-primary/10 text-foreground hover:bg-primary/15'
                        : 'bg-muted text-foreground hover:bg-muted/80',
                    )}
                  >
                    <span className="shrink-0 font-mono text-[10px] text-muted-foreground">
                      {ev.timeLabel}
                    </span>
                    <span className="truncate">{ev.schedule.text}</span>
                  </button>
                ))}
                {dayEvents.length > 3 ? (
                  <button
                    type="button"
                    onClick={() => setOverflowDay(cell.key)}
                    className="rounded-md px-1 py-0.5 text-left text-[10px] font-medium text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
                  >
                    +{dayEvents.length - 3} more
                  </button>
                ) : null}
              </div>
            </div>
          )
        })}
      </div>
      <Dialog
        open={overflowDay !== null}
        onOpenChange={open => {
          if (!open) setOverflowDay(null)
        }}
      >
        <DialogContent className="sm:max-w-sm">
          <DialogHeader>
            <DialogTitle>{overflowDay ?? ''}</DialogTitle>
          </DialogHeader>
          <ul className="flex max-h-80 flex-col divide-y divide-border/60 overflow-y-auto">
            {overflowEvents.map(ev => (
              <li key={ev.key}>
                <button
                  type="button"
                  onClick={() => {
                    setOverflowDay(null)
                    props.onSelectEvent(ev.schedule.id)
                  }}
                  className="flex w-full items-start gap-2.5 rounded-lg px-2 py-2 text-left transition-colors hover:bg-muted"
                >
                  <span className="mt-0.5 shrink-0 font-mono text-xs text-muted-foreground">
                    {ev.timeLabel}
                  </span>
                  <span className="min-w-0 flex-1 break-words text-sm [overflow-wrap:anywhere]">
                    {ev.schedule.text}
                  </span>
                </button>
              </li>
            ))}
          </ul>
        </DialogContent>
      </Dialog>
    </div>
  )
}
