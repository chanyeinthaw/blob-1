import type {
  CheckinConfig,
  ScheduleItem,
  ScheduleKind,
  ScheduleState,
  WatchItem,
  Weekday,
} from '@blob-1/contracts'
import { useAtomRefresh, useAtomSet, useAtomValue } from '@effect/atom-react'
import { Link } from '@tanstack/react-router'
import { AtomMachine } from '@typeonce/effect-machine/reactivity'
import { MachineState, useMachineResult } from '@typeonce/effect-machine-react'
import { AsyncResult } from 'effect/reactivity'
import { ChevronLeft, ChevronRight, Clock, Eye, Globe, Plus, Repeat, Trash2 } from 'lucide-react'
import { useCallback, useEffect, useMemo, useState } from 'react'

import { ConfirmDialog } from '~/components/ConfirmDialog'
import {
  EmptyLine,
  FieldError,
  FieldRow,
  focusRingClassName,
  PageError,
} from '~/components/layout/Page'
import { SplitDetail } from '~/components/layout/SplitPage'
import { Picker } from '~/components/Picker'
import { Button } from '~/components/ui/button'
import {
  Combobox,
  ComboboxContent,
  ComboboxEmpty,
  ComboboxItem,
  ComboboxList,
  ComboboxSearch,
  ComboboxTrigger,
} from '~/components/ui/combobox'
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '~/components/ui/dialog'
import { Input } from '~/components/ui/input'
import { Slider } from '~/components/ui/slider'
import { Switch } from '~/components/ui/switch'
import { Textarea } from '~/components/ui/textarea'
import { ToggleGroup, ToggleGroupItem } from '~/components/ui/toggle-group'
import { useBotInstanceKey, useBotsRoot } from '~/features/bots/CreateBotDialog'
import { machineAtomsForBot } from '~/lib/runtime'
import { createPersistentMachineContext } from '~/lib/shellMachines'
import { matchCached, useChangeRefresh } from '~/lib/useBackgroundRefresh'
import { cn } from '~/lib/utils'

import { MonthCalendar, WeekCalendar } from './CalendarGrid'
import {
  addDays,
  addMonths,
  buildMonthCells,
  buildWeekCells,
  cellsUtcRange,
  formatMonthTitle,
  formatWeekTitle,
  toZonedDate,
} from './calendarMath'
import { SchedulesEvents, type SchedulesInput, SchedulesMachine } from './schedulesMachine'
import { schedulesPageAtomFor } from './schedulesState'

const SchedulesOwner = createPersistentMachineContext(
  'schedules',
  ({ input, scopeKey }: { readonly input: SchedulesInput; readonly scopeKey: string }) =>
    machineAtomsForBot(scopeKey).make(SchedulesMachine, input),
)

const toggleItemClassName =
  'h-full rounded-full px-3 text-muted-foreground hover:bg-transparent hover:text-foreground aria-pressed:bg-background aria-pressed:text-foreground aria-pressed:shadow-sm'

type CalendarView = 'week' | 'month'

const ALL_WEEKDAYS: ReadonlyArray<{
  readonly id: Weekday
  readonly short: string
  readonly name: string
}> = [
  { id: 'sun', short: 'S', name: 'Sunday' },
  { id: 'mon', short: 'M', name: 'Monday' },
  { id: 'tue', short: 'T', name: 'Tuesday' },
  { id: 'wed', short: 'W', name: 'Wednesday' },
  { id: 'thu', short: 'T', name: 'Thursday' },
  { id: 'fri', short: 'F', name: 'Friday' },
  { id: 'sat', short: 'S', name: 'Saturday' },
]

const INTERVAL_PRESETS: ReadonlyArray<{ readonly seconds: number; readonly label: string }> = [
  { seconds: 900, label: '15m' },
  { seconds: 1800, label: '30m' },
  { seconds: 3600, label: '1h' },
  { seconds: 7200, label: '2h' },
  { seconds: 14400, label: '4h' },
]

const CRON_PRESETS: ReadonlyArray<{ readonly label: string; readonly cron: string }> = [
  { label: 'Daily 9am', cron: '0 9 * * *' },
  { label: 'Weekdays 9am', cron: '0 9 * * 1-5' },
  { label: 'Hourly', cron: '0 * * * *' },
]

const DEFAULT_CHECKIN: CheckinConfig = {
  every: 1800,
  days: ['mon', 'tue', 'wed', 'thu', 'fri'],
  startHour: 9,
  endHour: 22,
}

function formatHour(hour: number): string {
  return `${String(hour).padStart(2, '0')}:00`
}

function formatTimestamp(seconds: number, timeZone: string): string {
  try {
    return new Intl.DateTimeFormat(undefined, {
      timeZone,
      month: 'short',
      day: 'numeric',
      hour: '2-digit',
      minute: '2-digit',
      hour12: false,
    }).format(new Date(seconds * 1000))
  } catch {
    return new Date(seconds * 1000).toLocaleString()
  }
}

function formatInterval(seconds: number): string {
  if (seconds % 86400 === 0) return `${seconds / 86400}d`
  if (seconds % 3600 === 0) return `${seconds / 3600}h`
  return `${Math.round(seconds / 60)}m`
}

function listTimezones(): ReadonlyArray<string> {
  const zones = Intl.supportedValuesOf('timeZone')
  return zones.includes('UTC') ? zones : ['UTC', ...zones]
}

function initialWeekRange(timeZone: string) {
  const today = toZonedDate(Math.floor(Date.now() / 1000), timeZone)
  return cellsUtcRange(
    buildWeekCells({ year: today.year, month: today.month, day: today.day }, today.key),
  )
}

function SchedulesMount(props: {
  readonly botId: string
  readonly instanceKey: string
  readonly botName: string
  readonly state: ScheduleState
}) {
  const [range] = useState(() => initialWeekRange(props.state.timezone))
  return (
    <SchedulesOwner.Provider input={{ state: props.state, range }} scopeKey={props.instanceKey}>
      <ChangeRefreshWatcher instanceKey={props.instanceKey} />
      <SchedulesWorkspace
        botId={props.botId}
        botName={props.botName}
        initialTimezone={props.state.timezone}
      />
    </SchedulesOwner.Provider>
  )
}

interface SchedulesPageProps {
  readonly botId: string
}

export function SchedulesPage(props: SchedulesPageProps) {
  const instanceKey = useBotInstanceKey(props.botId)
  const atom = schedulesPageAtomFor(instanceKey)
  const loaded = useAtomValue(atom)
  const retry = useAtomRefresh(atom)
  const bots = useBotsRoot()?.bots ?? []
  const botName = bots.find(entry => entry.id === props.botId)?.name ?? props.botId

  return matchCached(loaded, {
    onInitial: () => (
      <SplitDetail
        title="Schedules"
        isListView={false}
        backTo="/settings/bots/$botId"
        backParams={{ botId: props.botId }}
        backLabel={botName}
      >
        <EmptyLine>Loading…</EmptyLine>
      </SplitDetail>
    ),
    onFailure: failure => (
      <SplitDetail
        title="Schedules"
        isListView={false}
        backTo="/settings/bots/$botId"
        backParams={{ botId: props.botId }}
        backLabel={botName}
      >
        <PageError cause={failure.cause} onRetry={retry} />
      </SplitDetail>
    ),
    onSuccess: value => (
      <SchedulesMount
        botId={props.botId}
        instanceKey={instanceKey}
        botName={botName}
        state={value}
      />
    ),
  })
}

interface SchedulesWorkspaceProps {
  readonly botId: string
  readonly botName: string
  readonly initialTimezone: string
}

interface CalendarToolbarProps {
  readonly botId: string
  readonly botName: string
  readonly title: string
  readonly timezone: string
  readonly view: CalendarView
  readonly jobIdle: boolean
  readonly hasMoreOccurrences: boolean
  readonly rangeLoading: boolean
  readonly onToday: () => void
  readonly onPrev: () => void
  readonly onNext: () => void
  readonly onChangeView: (view: CalendarView) => void
  readonly onLoadMore: () => void
  readonly onNew: () => void
}

function CalendarToolbar(props: CalendarToolbarProps) {
  return (
    <div className="flex shrink-0 flex-wrap items-center gap-2 border-b border-border/80 px-4 py-2.5">
      <Link
        to="/settings/bots/$botId"
        params={{ botId: props.botId }}
        aria-label={`Back to ${props.botName}`}
        className={cn(
          '-ml-1.5 grid size-8 shrink-0 place-items-center rounded-full text-muted-foreground transition-colors hover:bg-accent hover:text-foreground md:hidden',
          focusRingClassName,
        )}
      >
        <ChevronLeft className="size-4" />
      </Link>
      <Button size="sm" variant="secondary" onClick={props.onToday}>
        Today
      </Button>
      <div className="flex items-center">
        <Button
          size="icon-sm"
          variant="ghost"
          aria-label={props.view === 'week' ? 'Previous week' : 'Previous month'}
          onClick={props.onPrev}
        >
          <ChevronLeft />
        </Button>
        <Button
          size="icon-sm"
          variant="ghost"
          aria-label={props.view === 'week' ? 'Next week' : 'Next month'}
          onClick={props.onNext}
        >
          <ChevronRight />
        </Button>
      </div>
      <h1 className="mr-auto text-base font-semibold tracking-tight sm:text-lg">{props.title}</h1>
      {props.hasMoreOccurrences ? (
        <Button
          size="xs"
          variant="secondary"
          disabled={props.rangeLoading}
          onClick={props.onLoadMore}
        >
          {props.rangeLoading ? 'Loading…' : 'Load more'}
        </Button>
      ) : null}
      <TimezoneCombobox timezone={props.timezone} />
      <ToggleGroup
        aria-label="Calendar view"
        spacing={1}
        value={[props.view]}
        onValueChange={values => {
          const next = values[0]
          if ((next === 'week' || next === 'month') && next !== props.view) {
            props.onChangeView(next)
          }
        }}
        className="h-9 rounded-full bg-muted p-[3px] md:h-8"
      >
        <ToggleGroupItem value="week" className={toggleItemClassName}>
          Week
        </ToggleGroupItem>
        <ToggleGroupItem value="month" className={toggleItemClassName}>
          Month
        </ToggleGroupItem>
      </ToggleGroup>
      <Button size="sm" disabled={!props.jobIdle} onClick={props.onNew}>
        <Plus data-icon="inline-start" className="size-3.5" />
        New
      </Button>
    </div>
  )
}

function applyPreviewHours(
  checkin: CheckinConfig | null,
  previewHours: readonly [number, number] | null,
): CheckinConfig | null {
  if (checkin === null || previewHours === null) return checkin
  return { ...checkin, startHour: previewHours[0], endHour: previewHours[1] }
}

function buildVisibleGrid(
  view: CalendarView,
  anchor: { readonly year: number; readonly month: number; readonly day: number },
  todayKey: string,
) {
  const cells =
    view === 'week' ? buildWeekCells(anchor, todayKey) : buildMonthCells(anchor, todayKey)
  const title = view === 'week' ? formatWeekTitle(cells) : formatMonthTitle(anchor)
  return { cells, title }
}

function earliestDueTimestamp(data: ScheduleState): number | null {
  let earliest: number | null = data.nextCheckin
  for (const item of data.schedules) {
    if (earliest === null || item.nextDue < earliest) earliest = item.nextDue
  }
  for (const item of data.watches) {
    if (item.until !== null && (earliest === null || item.until < earliest)) {
      earliest = item.until
    }
  }
  return earliest
}

/** Refreshes the schedules machine when the server pushes a change (model tools, tasks, other tabs). */
function ChangeRefreshWatcher(props: { readonly instanceKey: string }) {
  const machine = SchedulesOwner.useMachine()
  const send = useAtomSet(machine.send)
  const refresh = useCallback(() => send(SchedulesEvents.Refresh()), [send])
  useChangeRefresh(props.instanceKey, 'schedule', refresh)
  return null
}

function DueRefreshWatcher(props: { readonly data: ScheduleState; readonly nowSeconds: number }) {
  const machine = SchedulesOwner.useMachine()
  const send = useAtomSet(machine.send)
  const earliestDue = earliestDueTimestamp(props.data)
  const { nowSeconds } = props
  useEffect(() => {
    if (earliestDue !== null && earliestDue <= nowSeconds) {
      send(SchedulesEvents.Refresh())
    }
  }, [earliestDue, nowSeconds, send])
  return null
}

/** Full edge-to-edge calendar paired with a right-hand check-ins, jobs & watches sidebar. */
function SchedulesWorkspace(props: SchedulesWorkspaceProps) {
  const machine = SchedulesOwner.useMachine()
  const send = useAtomSet(machine.send)
  const { initialTimezone } = props
  useEffect(() => {
    send(SchedulesEvents.Refresh())
    send(SchedulesEvents.LoadRange(initialWeekRange(initialTimezone)))
  }, [initialTimezone, send])
  const [nowSeconds, setNowSeconds] = useState(() => Math.floor(Date.now() / 1000))
  const [view, setView] = useState<CalendarView>('week')
  const [previewHours, setPreviewHours] = useState<readonly [number, number] | null>(null)
  const [anchor, setAnchor] = useState(() => {
    const initial = toZonedDate(Math.floor(Date.now() / 1000), props.initialTimezone)
    return { year: initial.year, month: initial.month, day: initial.day }
  })

  useEffect(() => {
    const timer = window.setInterval(() => {
      setNowSeconds(Math.floor(Date.now() / 1000))
    }, 60_000)
    return () => window.clearInterval(timer)
  }, [])

  const mutationIdle = AsyncResult.getOrElse(
    useMachineResult(AtomMachine.matches(machine, 'Mutation.Idle')),
    () => false,
  )
  const rangeLoading = AsyncResult.getOrElse(
    useMachineResult(AtomMachine.matches(machine, 'Range.Loading')),
    () => false,
  )

  const openSlot = (prefillWhen: string) =>
    send(SchedulesEvents.OpenScheduleDialog({ prefillWhen }))
  const openEvent = (id: string) => send(SchedulesEvents.SelectSchedule({ id }))

  return (
    <MachineState machine={machine} path="">
      {({ value, states }) => {
        const { data, occurrences } = value
        const today = toZonedDate(nowSeconds, data.timezone)
        const { cells, title } = buildVisibleGrid(view, anchor, today.key)
        const mutationErrors =
          states.Mutation.state.path === 'Mutation.Idle'
            ? states.Mutation.state.value
            : { jobError: null, checkinError: null, watchError: null }
        const selectedSchedule =
          data.schedules.find(item => item.id === value.selectedScheduleId) ?? null
        const jobDialogOpen = value.scheduleDialogOpen || selectedSchedule !== null

        const navigateTo = (
          nextAnchor: { readonly year: number; readonly month: number; readonly day: number },
          nextView: CalendarView,
        ) => {
          setAnchor(nextAnchor)
          const { cells: nextCells } = buildVisibleGrid(nextView, nextAnchor, today.key)
          send(SchedulesEvents.LoadRange(cellsUtcRange(nextCells)))
        }

        const liveCheckin = applyPreviewHours(data.checkin, previewHours)

        return (
          <div className="flex min-h-0 min-w-0 flex-1 flex-col bg-background md:flex-row">
            <DueRefreshWatcher data={data} nowSeconds={nowSeconds} />
            <div className="flex h-[46dvh] min-h-[300px] max-h-[420px] min-w-0 flex-col md:h-auto md:max-h-none md:min-h-0 md:flex-1">
              <CalendarToolbar
                botId={props.botId}
                botName={props.botName}
                title={title}
                timezone={data.timezone}
                view={view}
                jobIdle={mutationIdle}
                hasMoreOccurrences={value.nextOccurrenceCursor !== null}
                rangeLoading={rangeLoading}
                onToday={() =>
                  navigateTo({ year: today.year, month: today.month, day: today.day }, view)
                }
                onPrev={() =>
                  navigateTo(view === 'week' ? addDays(anchor, -7) : addMonths(anchor, -1), view)
                }
                onNext={() =>
                  navigateTo(view === 'week' ? addDays(anchor, 7) : addMonths(anchor, 1), view)
                }
                onChangeView={next => {
                  setView(next)
                  navigateTo(anchor, next)
                }}
                onLoadMore={() => send(SchedulesEvents.LoadMoreOccurrences())}
                onNew={() => send(SchedulesEvents.OpenScheduleDialog({ prefillWhen: null }))}
              />
              {view === 'week' ? (
                <WeekCalendar
                  cells={cells}
                  schedules={data.schedules}
                  occurrences={occurrences}
                  checkin={liveCheckin}
                  timezone={data.timezone}
                  nowSeconds={nowSeconds}
                  onSelectSlot={openSlot}
                  onSelectEvent={openEvent}
                />
              ) : (
                <MonthCalendar
                  cells={cells}
                  schedules={data.schedules}
                  occurrences={occurrences}
                  checkin={liveCheckin}
                  timezone={data.timezone}
                  onSelectSlot={openSlot}
                  onSelectEvent={openEvent}
                />
              )}
            </div>
            <SchedulesSidebar
              data={data}
              previewHours={previewHours}
              onPreviewHours={setPreviewHours}
              isIdle={mutationIdle}
              checkinError={mutationErrors.checkinError}
              jobError={!jobDialogOpen ? mutationErrors.jobError : null}
              watchError={!value.watchDialogOpen ? mutationErrors.watchError : null}
            />
            <ScheduleAddDialog
              key={
                value.scheduleDialogOpen
                  ? `open:${value.schedulePrefillWhen ?? ''}`
                  : 'schedule-closed'
              }
              open={value.scheduleDialogOpen}
              prefillWhen={value.schedulePrefillWhen}
              defaultTimezone={data.timezone}
              isIdle={mutationIdle}
              error={mutationErrors.jobError}
            />
            <ScheduleDetailDialog
              schedule={selectedSchedule}
              isIdle={mutationIdle}
              error={mutationErrors.jobError}
            />
            <WatchAddDialog
              key={value.watchDialogOpen ? 'watch-open' : 'watch-closed'}
              open={value.watchDialogOpen}
              isIdle={mutationIdle}
              error={mutationErrors.watchError}
            />
          </div>
        )
      }}
    </MachineState>
  )
}

interface SchedulesSidebarProps {
  readonly data: ScheduleState
  readonly previewHours: readonly [number, number] | null
  readonly onPreviewHours: (hours: readonly [number, number] | null) => void
  readonly isIdle: boolean
  readonly checkinError: string | null
  readonly jobError: string | null
  readonly watchError: string | null
}

function SchedulesSidebar(props: SchedulesSidebarProps) {
  const machine = SchedulesOwner.useMachine()
  const send = useAtomSet(machine.send)
  const { data } = props

  return (
    <aside className="flex w-full shrink-0 flex-col divide-y divide-border/70 border-t border-border/80 bg-background md:min-h-0 md:w-80 md:overflow-y-auto md:border-t-0 md:border-l lg:w-[340px]">
      <SidebarCheckins
        config={data.checkin}
        previewHours={props.previewHours}
        onPreviewHours={props.onPreviewHours}
        timezone={data.timezone}
        nextCheckin={data.nextCheckin}
        hasWatches={data.watches.length > 0}
        error={props.checkinError}
      />
      <div className="p-4">
        <div className="mb-2 flex items-center justify-between gap-2">
          <h2 className="text-sm font-semibold">Scheduled jobs</h2>
          <Button
            size="xs"
            variant="secondary"
            disabled={!props.isIdle}
            onClick={() => send(SchedulesEvents.OpenScheduleDialog({ prefillWhen: null }))}
          >
            <Plus data-icon="inline-start" className="size-3" />
            Add
          </Button>
        </div>
        {data.schedules.length === 0 ? (
          <p className="py-1.5 text-xs text-muted-foreground">No scheduled jobs.</p>
        ) : (
          <ul className="flex flex-col divide-y divide-border/60">
            {data.schedules.map(item => (
              <SidebarScheduleRow key={item.id} item={item} disabled={!props.isIdle} />
            ))}
          </ul>
        )}
        {props.jobError === null ? null : (
          <FieldError id="sidebar-job-error">{props.jobError}</FieldError>
        )}
      </div>
      <div className="p-4">
        <div className="mb-2 flex items-center justify-between gap-2">
          <h2 className="text-sm font-semibold">Watches</h2>
          <Button
            size="xs"
            variant="secondary"
            disabled={!props.isIdle}
            onClick={() => send(SchedulesEvents.OpenWatchDialog())}
          >
            <Plus data-icon="inline-start" className="size-3" />
            Add
          </Button>
        </div>
        {data.watches.length === 0 ? (
          <p className="py-1.5 text-xs text-muted-foreground">Nothing being watched.</p>
        ) : (
          <ul className="flex flex-col divide-y divide-border/60">
            {data.watches.map(item => (
              <SidebarWatchRow
                key={item.id}
                item={item}
                timezone={data.timezone}
                disabled={!props.isIdle}
              />
            ))}
          </ul>
        )}
        {props.watchError === null ? null : (
          <FieldError id="sidebar-watch-error">{props.watchError}</FieldError>
        )}
      </div>
    </aside>
  )
}

interface TimezoneComboboxProps {
  readonly timezone: string
}

function TimezoneCombobox(props: TimezoneComboboxProps) {
  const machine = SchedulesOwner.useMachine()
  const send = useAtomSet(machine.send)
  const zones = useMemo(() => listTimezones(), [])
  return (
    <Combobox
      items={zones}
      value={props.timezone}
      onValueChange={(next: string | null) => {
        if (next !== null && next !== props.timezone) {
          send(SchedulesEvents.SetTimezone({ timezone: next }))
        }
      }}
    >
      <ComboboxTrigger
        render={
          <Button
            size="xs"
            variant="ghost"
            aria-label={`Change timezone (${props.timezone})`}
            className="text-muted-foreground hover:text-foreground"
          />
        }
      >
        <Globe data-icon="inline-start" className="size-3.5" />
        <span className="max-w-36 truncate">{props.timezone}</span>
      </ComboboxTrigger>
      <ComboboxContent align="end" className="w-64 min-w-64">
        <ComboboxSearch placeholder="Search timezone…" />
        <ComboboxEmpty>No matches</ComboboxEmpty>
        <ComboboxList>
          {(tz: string) => (
            <ComboboxItem key={tz} value={tz}>
              <span className="truncate">{tz}</span>
            </ComboboxItem>
          )}
        </ComboboxList>
      </ComboboxContent>
    </Combobox>
  )
}

interface SidebarCheckinsProps {
  readonly config: CheckinConfig | null
  readonly previewHours: readonly [number, number] | null
  readonly onPreviewHours: (hours: readonly [number, number] | null) => void
  readonly timezone: string
  readonly nextCheckin: number | null
  readonly hasWatches: boolean
  readonly error: string | null
}

function SidebarCheckins(props: SidebarCheckinsProps) {
  const machine = SchedulesOwner.useMachine()
  const send = useAtomSet(machine.send)
  const { config, error } = props
  return (
    <div className="p-4">
      <div className="mb-2.5 flex items-center justify-between gap-2">
        <div className="flex items-center gap-2">
          <h2 className="text-sm font-semibold">Check-ins</h2>
          {config === null ? null : (
            <span aria-hidden="true" className="size-2 rounded-full bg-ok" />
          )}
        </div>
        <Switch
          aria-label="Enable check-ins"
          checked={config !== null}
          onCheckedChange={checked =>
            send(
              checked
                ? SchedulesEvents.SetCheckin({ config: DEFAULT_CHECKIN })
                : SchedulesEvents.ClearCheckin(),
            )
          }
        />
      </div>
      {config === null ? (
        <p className="py-1 text-xs text-muted-foreground">Check-ins are off.</p>
      ) : (
        <CheckinControls
          config={config}
          previewHours={props.previewHours}
          onPreviewHours={props.onPreviewHours}
          timezone={props.timezone}
          nextCheckin={props.nextCheckin}
          hasWatches={props.hasWatches}
          disabled={false}
        />
      )}
      {error === null ? null : <FieldError id="sidebar-checkin-error">{error}</FieldError>}
    </div>
  )
}

interface CheckinControlsProps {
  readonly config: CheckinConfig
  readonly previewHours: readonly [number, number] | null
  readonly onPreviewHours: (hours: readonly [number, number] | null) => void
  readonly timezone: string
  readonly nextCheckin: number | null
  readonly hasWatches: boolean
  readonly disabled: boolean
}

function CheckinControls(props: CheckinControlsProps) {
  const machine = SchedulesOwner.useMachine()
  const send = useAtomSet(machine.send)
  const { config, disabled } = props

  const update = (patch: Partial<CheckinConfig>) => {
    send(SchedulesEvents.SetCheckin({ config: { ...config, ...patch } }))
  }

  const toggleDay = (day: Weekday) => {
    const active = config.days.includes(day)
    if (active && config.days.length === 1) return
    const next = ALL_WEEKDAYS.map(item => item.id).filter(id =>
      id === day ? !active : config.days.includes(id),
    )
    update({ days: next })
  }

  const isPreset = INTERVAL_PRESETS.some(preset => preset.seconds === config.every)
  const activeHours: readonly [number, number] = props.previewHours ?? [
    config.startHour,
    config.endHour,
  ]

  return (
    <div className="space-y-3">
      <div className="space-y-1.5">
        <span id="checkin-every-label" className="text-xs font-medium text-muted-foreground">
          Every
        </span>
        <div className="flex flex-wrap items-center gap-1.5">
          <ToggleGroup
            aria-labelledby="checkin-every-label"
            spacing={1}
            disabled={disabled}
            value={[String(config.every)]}
            onValueChange={values => {
              const next = Number(values[0])
              if (Number.isFinite(next) && next >= 300 && next !== config.every) {
                update({ every: next })
              }
            }}
            className="h-8 rounded-full bg-muted p-[3px]"
          >
            {INTERVAL_PRESETS.map(preset => (
              <ToggleGroupItem
                key={preset.seconds}
                value={String(preset.seconds)}
                className="h-full rounded-full px-2.5 text-xs text-muted-foreground hover:bg-transparent hover:text-foreground aria-pressed:bg-background aria-pressed:text-foreground aria-pressed:shadow-sm"
              >
                {preset.label}
              </ToggleGroupItem>
            ))}
          </ToggleGroup>
          {isPreset ? null : (
            <span className="text-xs text-muted-foreground">{formatInterval(config.every)}</span>
          )}
        </div>
      </div>
      <div className="space-y-1.5">
        <span id="checkin-days-label" className="text-xs font-medium text-muted-foreground">
          Days
        </span>
        <fieldset aria-labelledby="checkin-days-label" className="flex items-center gap-1">
          {ALL_WEEKDAYS.map(day => {
            const active = config.days.includes(day.id)
            return (
              <button
                key={day.id}
                type="button"
                aria-label={day.name}
                aria-pressed={active}
                disabled={disabled}
                onClick={() => toggleDay(day.id)}
                className={cn(
                  'grid size-8 place-items-center rounded-full text-xs font-medium transition-colors disabled:opacity-50',
                  active
                    ? 'bg-primary text-primary-foreground'
                    : 'bg-muted text-muted-foreground hover:text-foreground',
                )}
              >
                {day.short}
              </button>
            )
          })}
        </fieldset>
      </div>
      <div className="space-y-1">
        <div className="flex items-center justify-between">
          <span id="checkin-hours-label" className="text-xs font-medium text-muted-foreground">
            Hours
          </span>
          <span className="font-mono text-xs text-foreground">
            {formatHour(activeHours[0])} – {formatHour(activeHours[1])}
          </span>
        </div>
        <Slider
          aria-labelledby="checkin-hours-label"
          thumbLabels={['Start hour', 'End hour']}
          min={0}
          max={24}
          step={1}
          minStepsBetweenValues={1}
          disabled={disabled}
          value={[activeHours[0], activeHours[1]]}
          onValueChange={val => {
            if (Array.isArray(val) && val.length === 2) {
              props.onPreviewHours([val[0], val[1]])
            }
          }}
          onValueCommitted={val => {
            props.onPreviewHours(null)
            if (Array.isArray(val) && val.length === 2 && val[0] < val[1]) {
              if (val[0] !== config.startHour || val[1] !== config.endHour) {
                update({ startHour: val[0], endHour: val[1] })
              }
            }
          }}
        />
      </div>
      <p className="text-xs text-muted-foreground">
        {props.hasWatches && props.nextCheckin !== null
          ? `Next · ${formatTimestamp(props.nextCheckin, props.timezone)}`
          : 'Runs when watches exist'}
      </p>
    </div>
  )
}

interface SidebarScheduleRowProps {
  readonly item: ScheduleItem
  readonly disabled: boolean
}

function SidebarScheduleRow(props: SidebarScheduleRowProps) {
  const machine = SchedulesOwner.useMachine()
  const send = useAtomSet(machine.send)
  const { item, disabled } = props
  const meta = [
    item.kind === 'cron' ? item.when : 'Once',
    formatTimestamp(item.nextDue, item.timezone),
  ].join(' · ')
  return (
    <li className="flex items-start gap-2 py-2">
      <button
        type="button"
        onClick={() => send(SchedulesEvents.SelectSchedule({ id: item.id }))}
        className="flex min-w-0 flex-1 items-start gap-2 text-left"
      >
        <span
          aria-hidden="true"
          className="mt-0.5 grid size-6 shrink-0 place-items-center rounded-full bg-muted text-muted-foreground"
        >
          {item.kind === 'cron' ? <Repeat className="size-3" /> : <Clock className="size-3" />}
        </span>
        <span className="min-w-0 flex-1">
          <span className="line-clamp-2 text-xs font-medium text-foreground">{item.text}</span>
          <span className="mt-0.5 block truncate text-[11px] text-muted-foreground">{meta}</span>
        </span>
      </button>
      <ConfirmDialog
        tooltip="Remove schedule"
        trigger={
          <Button
            variant="ghost"
            size="icon-xs"
            aria-label={`Remove schedule ${item.id}`}
            disabled={disabled}
            className="text-destructive hover:bg-destructive/10 hover:text-destructive"
          >
            <Trash2 />
          </Button>
        }
        title="Remove scheduled job?"
        detail={item.text}
        confirmLabel="Remove"
        onConfirm={() => send(SchedulesEvents.RemoveSchedule({ id: item.id }))}
      />
    </li>
  )
}

interface SidebarWatchRowProps {
  readonly item: WatchItem
  readonly timezone: string
  readonly disabled: boolean
}

function SidebarWatchRow(props: SidebarWatchRowProps) {
  const machine = SchedulesOwner.useMachine()
  const send = useAtomSet(machine.send)
  const { item, timezone, disabled } = props
  const [detailOpen, setDetailOpen] = useState(false)
  const until = item.until === null ? null : formatTimestamp(item.until, timezone)
  return (
    <li className="flex items-start gap-2 py-2">
      <button
        type="button"
        onClick={() => setDetailOpen(true)}
        className="flex min-w-0 flex-1 items-start gap-2 text-left"
      >
        <span
          aria-hidden="true"
          className="mt-0.5 grid size-6 shrink-0 place-items-center rounded-full bg-muted text-muted-foreground"
        >
          <Eye className="size-3" />
        </span>
        <span className="min-w-0 flex-1">
          <span className="line-clamp-2 text-xs font-medium text-foreground">{item.text}</span>
          {until === null ? null : (
            <span className="mt-0.5 block truncate text-[11px] text-muted-foreground">
              Until {until}
            </span>
          )}
        </span>
      </button>
      <ConfirmDialog
        tooltip="Remove watch"
        trigger={
          <Button
            variant="ghost"
            size="icon-xs"
            aria-label={`Remove watch ${item.id}`}
            disabled={disabled}
            className="text-destructive hover:bg-destructive/10 hover:text-destructive"
          >
            <Trash2 />
          </Button>
        }
        title="Remove watch?"
        detail={item.text}
        confirmLabel="Remove"
        onConfirm={() => send(SchedulesEvents.RemoveWatch({ id: item.id }))}
      />
      <Dialog open={detailOpen} onOpenChange={setDetailOpen}>
        <DialogContent className="sm:max-w-md">
          <DialogHeader>
            <DialogTitle>Watch</DialogTitle>
          </DialogHeader>
          <div className="space-y-2.5 text-sm">
            <p className="break-words whitespace-pre-wrap text-foreground [overflow-wrap:anywhere]">
              {item.text}
            </p>
            {until === null && item.target === null ? null : (
              <dl className="grid grid-cols-[88px_minmax(0,1fr)] gap-y-1.5 text-xs">
                {until === null ? null : (
                  <>
                    <dt className="text-muted-foreground">Until</dt>
                    <dd className="text-foreground">
                      {until} ({timezone})
                    </dd>
                  </>
                )}
                {item.target === null ? null : (
                  <>
                    <dt className="text-muted-foreground">Target</dt>
                    <dd className="text-foreground">{item.target}</dd>
                  </>
                )}
              </dl>
            )}
          </div>
          <DialogFooter>
            <Button type="button" variant="ghost" onClick={() => setDetailOpen(false)}>
              Close
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </li>
  )
}

interface ScheduleDetailDialogProps {
  readonly schedule: ScheduleItem | null
  readonly isIdle: boolean
  readonly error: string | null
}

function ScheduleDetailDialog(props: ScheduleDetailDialogProps) {
  const machine = SchedulesOwner.useMachine()
  const send = useAtomSet(machine.send)
  const { schedule, isIdle, error } = props

  return (
    <Dialog
      open={schedule !== null}
      onOpenChange={open => {
        if (!open && isIdle) send(SchedulesEvents.SelectSchedule({ id: null }))
      }}
    >
      <DialogContent className="sm:max-w-md">
        {schedule === null ? null : (
          <>
            <DialogHeader>
              <DialogTitle>
                {schedule.kind === 'cron' ? 'Recurring job' : 'Scheduled job'}
              </DialogTitle>
            </DialogHeader>
            <div className="space-y-2.5 text-sm">
              <p className="break-words text-foreground [overflow-wrap:anywhere]">
                {schedule.text}
              </p>
              <dl className="grid grid-cols-[88px_minmax(0,1fr)] gap-y-1.5 text-xs">
                <dt className="text-muted-foreground">Cadence</dt>
                <dd className="font-mono text-foreground">
                  {schedule.kind === 'cron' ? schedule.when : 'One-off'}
                </dd>
                <dt className="text-muted-foreground">Next run</dt>
                <dd className="text-foreground">
                  {formatTimestamp(schedule.nextDue, schedule.timezone)} ({schedule.timezone})
                </dd>
                {schedule.target === null ? null : (
                  <>
                    <dt className="text-muted-foreground">Target</dt>
                    <dd className="text-foreground">{schedule.target}</dd>
                  </>
                )}
              </dl>
              {error === null ? null : <FieldError id="schedule-detail-error">{error}</FieldError>}
            </div>
            <DialogFooter>
              <Button
                type="button"
                variant="destructive"
                disabled={!isIdle}
                onClick={() => send(SchedulesEvents.RemoveSchedule({ id: schedule.id }))}
              >
                {isIdle ? 'Remove' : 'Removing…'}
              </Button>
              <Button
                type="button"
                variant="ghost"
                disabled={!isIdle}
                onClick={() => send(SchedulesEvents.SelectSchedule({ id: null }))}
              >
                Close
              </Button>
            </DialogFooter>
          </>
        )}
      </DialogContent>
    </Dialog>
  )
}

interface ScheduleAddDialogProps {
  readonly open: boolean
  readonly prefillWhen: string | null
  readonly defaultTimezone: string
  readonly isIdle: boolean
  readonly error: string | null
}

function ScheduleAddDialog(props: ScheduleAddDialogProps) {
  const machine = SchedulesOwner.useMachine()
  const send = useAtomSet(machine.send)
  const [kind, setKind] = useState<ScheduleKind>(props.prefillWhen === null ? 'cron' : 'at')
  const [when, setWhen] = useState(props.prefillWhen ?? '0 9 * * 1-5')
  const [timezone, setTimezone] = useState(props.defaultTimezone)
  const [text, setText] = useState('')
  const tzOptions = useMemo(
    () => listTimezones().map(tz => ({ value: tz, label: tz, hint: null })),
    [],
  )

  const canSubmit = props.isIdle && text.trim() !== '' && when.trim() !== ''

  return (
    <Dialog
      open={props.open}
      onOpenChange={open => {
        if (!open && props.isIdle) send(SchedulesEvents.CloseScheduleDialog())
      }}
    >
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Add scheduled job</DialogTitle>
        </DialogHeader>
        <div className="space-y-1">
          <FieldRow labelId="schedule-kind-label" label="Type">
            <ToggleGroup
              aria-labelledby="schedule-kind-label"
              spacing={1}
              disabled={!props.isIdle}
              value={[kind]}
              onValueChange={values => {
                const next = values[0]
                if ((next === 'at' || next === 'cron') && next !== kind) {
                  setKind(next)
                  setWhen(next === 'cron' ? '0 9 * * 1-5' : (props.prefillWhen ?? ''))
                }
              }}
              className="h-10 rounded-full bg-muted p-[3px] md:h-8"
            >
              <ToggleGroupItem value="cron" className={toggleItemClassName}>
                Recurring
              </ToggleGroupItem>
              <ToggleGroupItem value="at" className={toggleItemClassName}>
                One-off
              </ToggleGroupItem>
            </ToggleGroup>
          </FieldRow>
          <FieldRow id="schedule-when" label="When">
            {kind === 'at' ? (
              <Input
                id="schedule-when"
                type="datetime-local"
                value={when}
                disabled={!props.isIdle}
                onChange={event => setWhen(event.target.value)}
              />
            ) : (
              <div className="space-y-2">
                <Input
                  id="schedule-when"
                  className="font-mono"
                  placeholder="0 9 * * 1-5"
                  value={when}
                  disabled={!props.isIdle}
                  onChange={event => setWhen(event.target.value)}
                />
                <div className="flex flex-wrap gap-1.5">
                  {CRON_PRESETS.map(preset => (
                    <Button
                      key={preset.cron}
                      type="button"
                      size="xs"
                      variant={when === preset.cron ? 'default' : 'secondary'}
                      disabled={!props.isIdle}
                      onClick={() => setWhen(preset.cron)}
                    >
                      {preset.label}
                    </Button>
                  ))}
                </div>
              </div>
            )}
          </FieldRow>
          <FieldRow id="schedule-add-tz" label="Timezone">
            <Picker
              id="schedule-add-tz"
              placeholder="Choose a timezone"
              value={timezone || props.defaultTimezone}
              disabled={!props.isIdle}
              options={tzOptions}
              onChange={setTimezone}
            />
          </FieldRow>
          <FieldRow id="schedule-text" label="Task">
            <Textarea
              id="schedule-text"
              rows={3}
              placeholder="What should Blob-1 do?"
              value={text}
              disabled={!props.isIdle}
              onChange={event => setText(event.target.value)}
            />
          </FieldRow>
          {props.error === null ? null : (
            <FieldError id="schedule-add-error">{props.error}</FieldError>
          )}
          <DialogFooter className="mt-4">
            <Button
              type="button"
              variant="ghost"
              disabled={!props.isIdle}
              onClick={() => send(SchedulesEvents.CloseScheduleDialog())}
            >
              Cancel
            </Button>
            <Button
              type="button"
              disabled={!canSubmit}
              onClick={() =>
                send(
                  SchedulesEvents.AddSchedule({
                    text: text.trim(),
                    kind,
                    when: when.trim(),
                    timezone: timezone || props.defaultTimezone || null,
                  }),
                )
              }
            >
              {props.isIdle ? 'Add' : 'Saving…'}
            </Button>
          </DialogFooter>
        </div>
      </DialogContent>
    </Dialog>
  )
}

interface WatchAddDialogProps {
  readonly open: boolean
  readonly isIdle: boolean
  readonly error: string | null
}

function WatchAddDialog(props: WatchAddDialogProps) {
  const machine = SchedulesOwner.useMachine()
  const send = useAtomSet(machine.send)
  const [text, setText] = useState('')
  const [until, setUntil] = useState('')
  const canSubmit = props.isIdle && text.trim() !== ''

  return (
    <Dialog
      open={props.open}
      onOpenChange={open => {
        if (!open && props.isIdle) send(SchedulesEvents.CloseWatchDialog())
      }}
    >
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Add watch</DialogTitle>
        </DialogHeader>
        <div className="space-y-1">
          <FieldRow id="watch-text" label="Watch">
            <Textarea
              id="watch-text"
              rows={3}
              placeholder="What should Blob-1 keep an eye on?"
              value={text}
              disabled={!props.isIdle}
              onChange={event => setText(event.target.value)}
            />
          </FieldRow>
          <FieldRow id="watch-until" label="Until">
            <Input
              id="watch-until"
              type="datetime-local"
              value={until}
              disabled={!props.isIdle}
              onChange={event => setUntil(event.target.value)}
            />
          </FieldRow>
          {props.error === null ? null : (
            <FieldError id="watch-add-error">{props.error}</FieldError>
          )}
          <DialogFooter className="mt-4">
            <Button
              type="button"
              variant="ghost"
              disabled={!props.isIdle}
              onClick={() => send(SchedulesEvents.CloseWatchDialog())}
            >
              Cancel
            </Button>
            <Button
              type="button"
              disabled={!canSubmit}
              onClick={() =>
                send(
                  SchedulesEvents.AddWatch({
                    text: text.trim(),
                    until: until.trim() === '' ? null : until.trim(),
                  }),
                )
              }
            >
              {props.isIdle ? 'Add' : 'Saving…'}
            </Button>
          </DialogFooter>
        </div>
      </DialogContent>
    </Dialog>
  )
}
