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

function applyScheduleState<
  T extends {
    readonly data: ScheduleState
    readonly mutationSeq: number
    readonly occurrencesVersion: number
  },
>(root: T, next: ScheduleState, isMutation: boolean): T {
  return {
    ...root,
    data: next,
    mutationSeq: isMutation ? root.mutationSeq + 1 : root.mutationSeq,
    occurrencesVersion: scheduleDefinitionsChanged(root.data.schedules, next.schedules)
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

const setTimezone = Effect.fn('Schedules.setTimezone')(function* (timezone: string) {
  const api = yield* Api
  const notify = yield* Notify
  return yield* api
    .request('schedule.setTimezone', { timezone })
    .pipe(Effect.catch(error => notify.error(error.message).pipe(Effect.as(null))))
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

const setCheckin = Effect.fn('Schedules.setCheckin')(function* (config: CheckinConfig) {
  const api = yield* Api
  return yield* api.request('checkin.set', config)
})

const clearCheckin = Effect.flatMap(Api, api => api.request('checkin.clear', {}))

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
    data: ScheduleState,
    occurrences: Schema.Array(ScheduleOccurrence),
    nextOccurrenceCursor: Schema.NullOr(ScheduleOccurrenceCursor),
    visibleRange: RangeSchema,
    /** Incremented on every successful mutation so delayed refreshes never overwrite newer saves. */
    mutationSeq: Schema.Finite,
    /** Incremented when schedule definitions change so `Range` reloads the visible range. */
    occurrencesVersion: Schema.Finite,
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
    /** Serializes all schedule/check-in/watch/timezone mutations so full-state
     *  replies are never applied out of order across regions. */
    Mutation: {
      states: {
        Idle: {
          fields: {
            jobError: ErrorMessage,
            checkinError: ErrorMessage,
            watchError: ErrorMessage,
          },
        },
        SettingTimezone: { fields: { timezone: Schema.String } },
        AddingJob: {
          fields: {
            text: Schema.String,
            kind: ScheduleKind,
            when: Schema.String,
            timezone: Schema.NullOr(Schema.String),
          },
        },
        RemovingJob: { fields: { id: Schema.String } },
        SettingCheckin: { fields: { config: CheckinConfig } },
        ClearingCheckin: {},
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
    setTimezone,
    fetchOccurrenceBatch,
    refreshScheduleState,
    addSchedule,
    removeSchedule,
    setCheckin,
    clearCheckin,
    addWatch,
    removeWatch,
  },
}).handle({
  root: ({ input }) => ({
    data: input.state,
    occurrences: [],
    nextOccurrenceCursor: null,
    visibleRange: input.range,
    mutationSeq: 0,
    occurrencesVersion: 0,
    scheduleDialogOpen: false,
    schedulePrefillWhen: null,
    selectedScheduleId: null,
    watchDialogOpen: false,
  }),
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
          on: {
            Refresh: {
              target: 'Sync.Working',
              data: ({ root }) => ({ mutationSeq: root.mutationSeq }),
            },
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
                  output === null || state.mutationSeq !== root.mutationSeq
                    ? root
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
            SetTimezone: {
              target: 'Mutation.SettingTimezone',
              data: ({ event }) => ({ timezone: event.timezone }),
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
            SetCheckin: {
              target: 'Mutation.SettingCheckin',
              data: ({ event }) => ({ config: event.config }),
            },
            ClearCheckin: {
              target: 'Mutation.ClearingCheckin',
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
        SettingTimezone: {
          invoke: {
            src: 'setTimezone',
            input: ({ state }) => state.timezone,
            onDone: {
              target: 'Mutation.Idle',
              update: 'root',
              data: ({ root, output }) => ({
                target: noErrors,
                update: output === null ? root : applyScheduleState(root, output, true),
              }),
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
        SettingCheckin: {
          invoke: {
            src: 'setCheckin',
            input: ({ state }) => state.config,
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
              data: ({ error }) => ({ ...noErrors, checkinError: error.message }),
            },
          },
        },
        ClearingCheckin: {
          invoke: {
            src: 'clearCheckin',
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
              data: ({ error }) => ({ ...noErrors, checkinError: error.message }),
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
