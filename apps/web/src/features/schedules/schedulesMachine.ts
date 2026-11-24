import { Api } from '@blob-1/client/api'
import {
  CheckinConfig,
  type ScheduleItem,
  ScheduleKind,
  ScheduleOccurrence,
  ScheduleOccurrenceCursor,
  type ScheduleOccurrencesPage,
  ScheduleState,
} from '@blob-1/contracts'
import { Machine } from '@typeonce/effect-machine'
import { Effect, Schema } from 'effect'

import { Notify } from '~/lib/notify'

const MAX_OCCURRENCE_PAGES = 10

const RangeSchema = Schema.Struct({
  start: Schema.Finite,
  end: Schema.Finite,
})

const PendingCheckin = Schema.Union([
  Schema.Struct({ kind: Schema.Literal('set'), config: CheckinConfig }),
  Schema.Struct({ kind: Schema.Literal('clear') }),
])
type PendingCheckin = typeof PendingCheckin.Type

/** Returns true only when schedule definitions or one-off firing times changed,
 *  so a background refresh advancing a recurring job's `nextDue` does not reset
 *  paginated calendar occurrences. */
function scheduleDefinitionsChanged(
  prev: ReadonlyArray<ScheduleItem>,
  next: ReadonlyArray<ScheduleItem>,
): boolean {
  if (prev.length !== next.length) return true
  const byId = new Map(prev.map(item => [item.id, item]))
  for (const item of next) {
    const before = byId.get(item.id)
    if (
      before === undefined ||
      before.kind !== item.kind ||
      before.when !== item.when ||
      before.timezone !== item.timezone ||
      (item.kind === 'at' && before.nextDue !== item.nextDue)
    ) {
      return true
    }
  }
  return false
}

/** The check-in to show: a queued or in-flight local edit wins over the server's. */
function effectiveCheckin(
  pending: PendingCheckin | null,
  inFlight: PendingEdit | null,
  server: ScheduleState['checkin'],
): ScheduleState['checkin'] {
  if (pending !== null) return pending.kind === 'set' ? pending.config : null
  if (inFlight?.kind === 'setCheckin') return inFlight.config
  if (inFlight?.kind === 'clearCheckin') return null
  return server
}

function applyScheduleState<
  T extends {
    readonly serverData: ScheduleState
    readonly data: ScheduleState
    readonly pendingTimezone: string | null
    readonly pendingCheckin: PendingCheckin | null
    readonly inFlightEdit: PendingEdit | null
    readonly mutationSeq: number
    readonly occurrencesVersion: number
    readonly refreshQueued: boolean
  },
>(root: T, next: ScheduleState, isMutation: boolean): T {
  const effectiveTimezone =
    root.pendingTimezone ??
    (root.inFlightEdit?.kind === 'timezone' ? root.inFlightEdit.timezone : next.timezone)
  return {
    ...root,
    serverData: next,
    data: {
      ...next,
      timezone: effectiveTimezone,
      checkin: effectiveCheckin(root.pendingCheckin, root.inFlightEdit, next.checkin),
    },
    mutationSeq: isMutation ? root.mutationSeq + 1 : root.mutationSeq,
    // A write's reply can be older than a snapshot fetched after a push
    // while it was in flight; one follow-up fetch reconciles them.
    refreshQueued: isMutation || root.refreshQueued,
    occurrencesVersion: scheduleDefinitionsChanged(root.serverData.schedules, next.schedules)
      ? root.occurrencesVersion + 1
      : root.occurrencesVersion,
  }
}

/** Pages through `schedule.occurrences` from `cursor` (up to 10 pages / 5,000 items per batch). */
const fetchOccurrenceBatch = Effect.fn('Schedules.fetchOccurrenceBatch')(function* (input: {
  readonly start: number
  readonly end: number
  readonly cursor: ScheduleOccurrenceCursor | null
}) {
  const api = yield* Api
  const notify = yield* Notify
  return yield* Effect.gen(function* () {
    const collected: Array<ScheduleOccurrence> = []
    let cursor = input.cursor
    for (let page = 0; page < MAX_OCCURRENCE_PAGES; page++) {
      const reply: ScheduleOccurrencesPage = yield* api.request('schedule.occurrences', {
        start: input.start,
        end: input.end,
        cursor,
      })
      collected.push(...reply.occurrences)
      cursor = reply.nextCursor
      if (cursor === null) break
    }
    const occurrences: ReadonlyArray<ScheduleOccurrence> = collected
    return { occurrences, nextCursor: cursor }
  }).pipe(Effect.catch(error => notify.error(error.message).pipe(Effect.as(null))))
})

const PendingEdit = Schema.Union([
  Schema.Struct({ kind: Schema.Literal('timezone'), timezone: Schema.String }),
  Schema.Struct({ kind: Schema.Literal('setCheckin'), config: CheckinConfig }),
  Schema.Struct({ kind: Schema.Literal('clearCheckin') }),
])
type PendingEdit = typeof PendingEdit.Type

const flushPendingEdit = Effect.fn('Schedules.flushPendingEdit')(function* (edit: PendingEdit) {
  const api = yield* Api
  const notify = yield* Notify
  const req =
    edit.kind === 'timezone'
      ? api.request('schedule.setTimezone', { timezone: edit.timezone })
      : edit.kind === 'setCheckin'
        ? api.request('checkin.set', edit.config)
        : api.request('checkin.clear', {})
  return yield* req.pipe(Effect.catch(error => notify.error(error.message).pipe(Effect.as(null))))
})

const addSchedule = Effect.fn('Schedules.addSchedule')(function* (input: {
  readonly text: string
  readonly kind: ScheduleKind
  readonly when: string
  readonly timezone: string | null
}) {
  const api = yield* Api
  return yield* api.request('schedule.add', input)
})

const removeSchedule = Effect.fn('Schedules.removeSchedule')(function* (id: string) {
  const api = yield* Api
  return yield* api.request('schedule.remove', { id })
})

const refreshScheduleState = Effect.gen(function* () {
  const api = yield* Api
  const notify = yield* Notify
  return yield* api
    .request('schedule.get', {})
    .pipe(Effect.catch(error => notify.error(error.message).pipe(Effect.as(null))))
})

const addWatch = Effect.fn('Schedules.addWatch')(function* (input: {
  readonly text: string
  readonly until: string | null
}) {
  const api = yield* Api
  return yield* api.request('watch.add', input)
})

const removeWatch = Effect.fn('Schedules.removeWatch')(function* (id: string) {
  const api = yield* Api
  return yield* api.request('watch.remove', { id })
})

export const SchedulesInput = Schema.Struct({
  state: ScheduleState,
  range: RangeSchema,
})
export type SchedulesInput = typeof SchedulesInput.Type

const ErrorMessage = Schema.NullOr(Schema.String)

const noErrors = { jobError: null, checkinError: null, watchError: null }

export const SchedulesStates = Machine.state({
  type: 'parallel',
  fields: {
    serverData: ScheduleState,
    data: ScheduleState,
    pendingTimezone: Schema.NullOr(Schema.String),
    pendingCheckin: Schema.NullOr(PendingCheckin),
    inFlightEdit: Schema.NullOr(PendingEdit),
    occurrences: Schema.Array(ScheduleOccurrence),
    nextOccurrenceCursor: Schema.NullOr(ScheduleOccurrenceCursor),
    visibleRange: RangeSchema,
    /** Incremented on every successful mutation so delayed refreshes never overwrite newer saves. */
    mutationSeq: Schema.Finite,
    /** Incremented when schedule definitions change so `Range` reloads the visible range. */
    occurrencesVersion: Schema.Finite,
    /** A refresh was requested; `Sync` runs it as soon as no fetch is in flight. */
    refreshQueued: Schema.Boolean,
    scheduleDialogOpen: Schema.Boolean,
    schedulePrefillWhen: Schema.NullOr(Schema.String),
    selectedScheduleId: Schema.NullOr(Schema.String),
    watchDialogOpen: Schema.Boolean,
  },
  states: {
    Range: {
      states: {
        Idle: { fields: { loadedVersion: Schema.Finite } },
        Loading: {
          fields: {
            start: Schema.Finite,
            end: Schema.Finite,
            cursor: Schema.NullOr(ScheduleOccurrenceCursor),
            append: Schema.Boolean,
            loadedVersion: Schema.Finite,
          },
        },
      },
    },
    Sync: {
      states: {
        Idle: {},
        Working: { fields: { mutationSeq: Schema.Finite } },
      },
    },
    /** Serializes server writes while optimistic timezone and check-in edits coalesce
     *  immediately on root state without disabling controls. */
    Mutation: {
      states: {
        Idle: {
          fields: {
            jobError: ErrorMessage,
            checkinError: ErrorMessage,
            watchError: ErrorMessage,
          },
        },
        Flushing: { fields: { edit: PendingEdit } },
        AddingJob: {
          fields: {
            text: Schema.String,
            kind: ScheduleKind,
            when: Schema.String,
            timezone: Schema.NullOr(Schema.String),
          },
        },
        RemovingJob: { fields: { id: Schema.String } },
        AddingWatch: {
          fields: {
            text: Schema.String,
            until: Schema.NullOr(Schema.String),
          },
        },
        RemovingWatch: { fields: { id: Schema.String } },
      },
    },
  },
})

export const SchedulesEvents = Machine.events({
  SetTimezone: { timezone: Schema.String },
  LoadRange: { start: Schema.Finite, end: Schema.Finite },
  LoadMoreOccurrences: {},
  Refresh: {},
  OpenScheduleDialog: { prefillWhen: Schema.NullOr(Schema.String) },
  CloseScheduleDialog: {},
  SelectSchedule: { id: Schema.NullOr(Schema.String) },
  AddSchedule: {
    text: Schema.String,
    kind: ScheduleKind,
    when: Schema.String,
    timezone: Schema.NullOr(Schema.String),
  },
  RemoveSchedule: { id: Schema.String },
  SetCheckin: { config: CheckinConfig },
  ClearCheckin: {},
  OpenWatchDialog: {},
  CloseWatchDialog: {},
  AddWatch: { text: Schema.String, until: Schema.NullOr(Schema.String) },
  RemoveWatch: { id: Schema.String },
})

export const SchedulesMachine = Machine.make({
  id: 'Schedules',
  root: SchedulesStates,
  events: SchedulesEvents,
  input: SchedulesInput,
  effects: {
    flushPendingEdit,
    fetchOccurrenceBatch,
    refreshScheduleState,
    addSchedule,
    removeSchedule,
    addWatch,
    removeWatch,
  },
}).handle({
  root: ({ input }) => ({
    serverData: input.state,
    data: input.state,
    pendingTimezone: null,
    pendingCheckin: null,
    inFlightEdit: null,
    occurrences: [],
    nextOccurrenceCursor: null,
    visibleRange: input.range,
    mutationSeq: 0,
    occurrencesVersion: 0,
    refreshQueued: false,
    scheduleDialogOpen: false,
    schedulePrefillWhen: null,
    selectedScheduleId: null,
    watchDialogOpen: false,
  }),
  on: {
    // Queued rather than handled by `Sync.Idle`, so a push that lands mid-fetch
    // still triggers one trailing refresh instead of being dropped.
    Refresh: {
      update: 'root',
      data: ({ root }) => ({ ...root, refreshQueued: true }),
    },
    SetTimezone: {
      update: 'root',
      data: ({ root, event }) => ({
        ...root,
        pendingTimezone: event.timezone,
        mutationSeq: root.mutationSeq + 1,
        data: { ...root.data, timezone: event.timezone },
      }),
    },
    SetCheckin: {
      update: 'root',
      data: ({ root, event }) => ({
        ...root,
        pendingCheckin: { kind: 'set', config: event.config },
        mutationSeq: root.mutationSeq + 1,
        data: { ...root.data, checkin: event.config },
      }),
    },
    ClearCheckin: {
      update: 'root',
      data: ({ root }) => ({
        ...root,
        pendingCheckin: { kind: 'clear' },
        mutationSeq: root.mutationSeq + 1,
        data: { ...root.data, checkin: null, nextCheckin: null },
      }),
    },
  },
  states: {
    Range: {
      initial: {
        target: 'Range.Loading',
        data: ({ root }) => ({
          start: root.visibleRange.start,
          end: root.visibleRange.end,
          cursor: null,
          append: false,
          loadedVersion: root.occurrencesVersion,
        }),
      },
      states: {
        Idle: {
          always: {
            guard: ({ root, state }) => state.loadedVersion !== root.occurrencesVersion,
            target: 'Range.Loading',
            update: 'root',
            data: ({ root }) => ({
              target: {
                start: root.visibleRange.start,
                end: root.visibleRange.end,
                cursor: null,
                append: false,
                loadedVersion: root.occurrencesVersion,
              },
              update: { ...root, nextOccurrenceCursor: null },
            }),
          },
          on: {
            LoadRange: {
              target: 'Range.Loading',
              update: 'root',
              data: ({ root, event }) => ({
                target: {
                  start: event.start,
                  end: event.end,
                  cursor: null,
                  append: false,
                  loadedVersion: root.occurrencesVersion,
                },
                update: {
                  ...root,
                  visibleRange: { start: event.start, end: event.end },
                  nextOccurrenceCursor: null,
                },
              }),
            },
            LoadMoreOccurrences: {
              guard: ({ root }) => root.nextOccurrenceCursor !== null,
              target: 'Range.Loading',
              data: ({ root }) => ({
                start: root.visibleRange.start,
                end: root.visibleRange.end,
                cursor: root.nextOccurrenceCursor,
                append: true,
                loadedVersion: root.occurrencesVersion,
              }),
            },
          },
        },
        Loading: {
          on: {
            LoadRange: {
              target: 'Range.Loading',
              reenter: true,
              update: 'root',
              data: ({ root, event }) => ({
                target: {
                  start: event.start,
                  end: event.end,
                  cursor: null,
                  append: false,
                  loadedVersion: root.occurrencesVersion,
                },
                update: {
                  ...root,
                  visibleRange: { start: event.start, end: event.end },
                  nextOccurrenceCursor: null,
                },
              }),
            },
          },
          invoke: {
            src: 'fetchOccurrenceBatch',
            input: ({ state }) => ({
              start: state.start,
              end: state.end,
              cursor: state.cursor,
            }),
            onDone: {
              target: 'Range.Idle',
              update: 'root',
              data: ({ root, state, output }) => ({
                target: { loadedVersion: state.loadedVersion },
                update:
                  output === null ||
                  state.loadedVersion !== root.occurrencesVersion ||
                  state.start !== root.visibleRange.start ||
                  state.end !== root.visibleRange.end
                    ? root
                    : {
                        ...root,
                        occurrences: state.append
                          ? [...root.occurrences, ...output.occurrences]
                          : output.occurrences,
                        nextOccurrenceCursor: output.nextCursor,
                      },
              }),
            },
          },
        },
      },
    },
    Sync: {
      initial: { target: 'Sync.Idle' },
      states: {
        Idle: {
          // Waits out local edits so the fetch can't race an in-flight write.
          always: {
            guard: ({ root }) =>
              root.refreshQueued &&
              root.inFlightEdit === null &&
              root.pendingTimezone === null &&
              root.pendingCheckin === null,
            target: 'Sync.Working',
            update: 'root',
            data: ({ root }) => ({
              target: { mutationSeq: root.mutationSeq },
              update: { ...root, refreshQueued: false },
            }),
          },
        },
        Working: {
          invoke: {
            src: 'refreshScheduleState',
            onDone: {
              target: 'Sync.Idle',
              update: 'root',
              data: ({ root, state, output }) => ({
                target: undefined,
                update:
                  output === null
                    ? root
                    : state.mutationSeq !== root.mutationSeq
                      ? // A local edit superseded this snapshot; fetch again once it settles.
                        { ...root, refreshQueued: true }
                      : applyScheduleState(root, output, false),
              }),
            },
          },
        },
      },
    },
    Mutation: {
      initial: { target: 'Mutation.Idle', data: noErrors },
      states: {
        Idle: {
          always: {
            guard: ({ root }) => root.pendingTimezone !== null || root.pendingCheckin !== null,
            target: 'Mutation.Flushing',
            update: 'root',
            data: ({ root }) => {
              if (root.pendingTimezone !== null) {
                const edit: PendingEdit = { kind: 'timezone', timezone: root.pendingTimezone }
                return {
                  target: { edit },
                  update: { ...root, pendingTimezone: null, inFlightEdit: edit },
                }
              }
              const pending = root.pendingCheckin
              const edit: PendingEdit =
                pending?.kind === 'set'
                  ? { kind: 'setCheckin', config: pending.config }
                  : { kind: 'clearCheckin' }
              return {
                target: { edit },
                update: { ...root, pendingCheckin: null, inFlightEdit: edit },
              }
            },
          },
          on: {
            OpenScheduleDialog: {
              target: 'Mutation.Idle',
              update: 'root',
              data: ({ root, event }) => ({
                target: noErrors,
                update: {
                  ...root,
                  scheduleDialogOpen: true,
                  schedulePrefillWhen: event.prefillWhen,
                },
              }),
            },
            CloseScheduleDialog: {
              target: 'Mutation.Idle',
              update: 'root',
              data: ({ root }) => ({
                target: noErrors,
                update: { ...root, scheduleDialogOpen: false, schedulePrefillWhen: null },
              }),
            },
            SelectSchedule: {
              target: 'Mutation.Idle',
              update: 'root',
              data: ({ root, event }) => ({
                target: noErrors,
                update: { ...root, selectedScheduleId: event.id },
              }),
            },
            OpenWatchDialog: {
              target: 'Mutation.Idle',
              update: 'root',
              data: ({ root }) => ({
                target: noErrors,
                update: { ...root, watchDialogOpen: true },
              }),
            },
            CloseWatchDialog: {
              target: 'Mutation.Idle',
              update: 'root',
              data: ({ root }) => ({
                target: noErrors,
                update: { ...root, watchDialogOpen: false },
              }),
            },
            AddSchedule: {
              target: 'Mutation.AddingJob',
              data: ({ event }) => ({
                text: event.text,
                kind: event.kind,
                when: event.when,
                timezone: event.timezone,
              }),
            },
            RemoveSchedule: {
              target: 'Mutation.RemovingJob',
              data: ({ event }) => ({ id: event.id }),
            },
            AddWatch: {
              target: 'Mutation.AddingWatch',
              data: ({ event }) => ({ text: event.text, until: event.until }),
            },
            RemoveWatch: {
              target: 'Mutation.RemovingWatch',
              data: ({ event }) => ({ id: event.id }),
            },
          },
        },
        Flushing: {
          invoke: {
            src: 'flushPendingEdit',
            input: ({ state }) => state.edit,
            onDone: {
              target: 'Mutation.Idle',
              update: 'root',
              data: ({ root, state, output }) => {
                const cleared = { ...root, inFlightEdit: null }
                if (output !== null) {
                  return { target: noErrors, update: applyScheduleState(cleared, output, true) }
                }
                const rolledBack =
                  state.edit.kind === 'timezone'
                    ? {
                        ...cleared.data,
                        timezone: cleared.pendingTimezone ?? cleared.serverData.timezone,
                      }
                    : cleared.pendingCheckin !== null
                      ? cleared.data
                      : {
                          ...cleared.data,
                          checkin: cleared.serverData.checkin,
                          nextCheckin: cleared.serverData.nextCheckin,
                        }
                return { target: noErrors, update: { ...cleared, data: rolledBack } }
              },
            },
          },
        },
        AddingJob: {
          invoke: {
            src: 'addSchedule',
            input: ({ state }) => ({
              text: state.text,
              kind: state.kind,
              when: state.when,
              timezone: state.timezone,
            }),
            onDone: {
              target: 'Mutation.Idle',
              update: 'root',
              data: ({ root, output }) => ({
                target: noErrors,
                update: {
                  ...applyScheduleState(root, output, true),
                  scheduleDialogOpen: false,
                  schedulePrefillWhen: null,
                },
              }),
            },
            onFailure: {
              target: 'Mutation.Idle',
              data: ({ error }) => ({ ...noErrors, jobError: error.message }),
            },
          },
        },
        RemovingJob: {
          invoke: {
            src: 'removeSchedule',
            input: ({ state }) => state.id,
            onDone: {
              target: 'Mutation.Idle',
              update: 'root',
              data: ({ root, state, output }) => ({
                target: noErrors,
                update: {
                  ...applyScheduleState(root, output, true),
                  selectedScheduleId:
                    root.selectedScheduleId === state.id ? null : root.selectedScheduleId,
                },
              }),
            },
            onFailure: {
              target: 'Mutation.Idle',
              data: ({ error }) => ({ ...noErrors, jobError: error.message }),
            },
          },
        },
        AddingWatch: {
          invoke: {
            src: 'addWatch',
            input: ({ state }) => ({ text: state.text, until: state.until }),
            onDone: {
              target: 'Mutation.Idle',
              update: 'root',
              data: ({ root, output }) => ({
                target: noErrors,
                update: {
                  ...applyScheduleState(root, output, true),
                  watchDialogOpen: false,
                },
              }),
            },
            onFailure: {
              target: 'Mutation.Idle',
              data: ({ error }) => ({ ...noErrors, watchError: error.message }),
            },
          },
        },
        RemovingWatch: {
          invoke: {
            src: 'removeWatch',
            input: ({ state }) => state.id,
            onDone: {
              target: 'Mutation.Idle',
              update: 'root',
              data: ({ root, output }) => ({
                target: noErrors,
                update: applyScheduleState(root, output, true),
              }),
            },
            onFailure: {
              target: 'Mutation.Idle',
              data: ({ error }) => ({ ...noErrors, watchError: error.message }),
            },
          },
        },
      },
    },
  },
})
