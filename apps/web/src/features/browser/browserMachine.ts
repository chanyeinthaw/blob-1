import { Api } from '@blob-1/client/api'
import { BrowserStatus } from '@blob-1/contracts'
import { Machine } from '@typeonce/effect-machine'
import { Effect, Schema } from 'effect'

import { Notify } from '~/lib/notify'

const fetchBrowserStatus = Effect.gen(function* () {
  const api = yield* Api
  const notify = yield* Notify
  return yield* api
    .request('browser.status', {})
    .pipe(Effect.catch(error => notify.error(error.message).pipe(Effect.as(null))))
})

const addProfile = Effect.fn('Browser.addProfile')(function* (name: string) {
  const api = yield* Api
  return yield* api.request('browser.profiles.add', { name })
})

const removeProfile = Effect.fn('Browser.removeProfile')(function* (name: string) {
  const api = yield* Api
  const notify = yield* Notify
  return yield* api
    .request('browser.profiles.remove', { name })
    .pipe(Effect.catch(error => notify.error(error.message).pipe(Effect.as(null))))
})

export const BrowserInput = BrowserStatus
export type BrowserInput = typeof BrowserInput.Type

const ErrorMessage = Schema.NullOr(Schema.String)

export const BrowserStates = Machine.state({
  type: 'parallel',
  fields: {
    data: BrowserStatus,
    mutationSeq: Schema.Finite,
  },
  states: {
    Sync: {
      states: {
        Idle: {},
        Refreshing: { fields: { mutationSeq: Schema.Finite } },
      },
    },
    Add: {
      states: {
        Closed: {},
        Editing: { fields: { error: ErrorMessage } },
        Submitting: { fields: { name: Schema.String } },
      },
    },
    Remove: {
      states: {
        Idle: {},
        Working: { fields: { name: Schema.String } },
      },
    },
  },
})

export const BrowserEvents = Machine.events({
  Refresh: {},
  OpenAdd: {},
  CloseAdd: {},
  ClearAddError: {},
  SubmitAdd: { name: Schema.String },
  RemoveProfile: { name: Schema.String },
})

/** Merges a whole-status reply into root; `null` (a notified failure) keeps the current status. */
const mergeReply = (
  root: {
    readonly data: BrowserStatus
    readonly mutationSeq: number
  },
  output: BrowserStatus | null,
) => ({
  ...root,
  data: output ?? root.data,
  mutationSeq: root.mutationSeq + 1,
})

/** Manages browser profiles: refresh, add with inline errors, and removal. */
export const BrowserMachine = Machine.make({
  id: 'Browser',
  root: BrowserStates,
  events: BrowserEvents,
  input: BrowserInput,
  effects: { fetchBrowserStatus, addProfile, removeProfile },
}).handle({
  root: ({ input }) => ({ data: input, mutationSeq: 0 }),
  states: {
    Sync: {
      initial: { target: 'Sync.Idle' },
      states: {
        Idle: {
          on: {
            Refresh: {
              target: 'Sync.Refreshing',
              data: ({ root }) => ({ mutationSeq: root.mutationSeq }),
            },
          },
        },
        Refreshing: {
          invoke: {
            src: 'fetchBrowserStatus',
            onDone: {
              target: 'Sync.Idle',
              update: 'root',
              data: ({ root, state, output }) => ({
                target: undefined,
                update:
                  output === null || state.mutationSeq !== root.mutationSeq
                    ? root
                    : { ...root, data: output },
              }),
            },
          },
        },
      },
    },
    Add: {
      initial: { target: 'Add.Closed' },
      states: {
        Closed: {
          on: {
            OpenAdd: { target: 'Add.Editing', data: { error: null } },
          },
        },
        Editing: {
          on: {
            CloseAdd: { target: 'Add.Closed' },
            ClearAddError: { update: 'Add.Editing', data: { error: null } },
            SubmitAdd: {
              target: 'Add.Submitting',
              update: 'root',
              data: ({ root, event }) => ({
                target: { name: event.name },
                update: { ...root, mutationSeq: root.mutationSeq + 1 },
              }),
            },
          },
        },
        Submitting: {
          invoke: {
            src: 'addProfile',
            input: ({ state }) => state.name,
            onDone: {
              target: 'Add.Closed',
              update: 'root',
              data: ({ root, output }) => ({ target: undefined, update: mergeReply(root, output) }),
            },
            onFailure: {
              target: 'Add.Editing',
              data: ({ error }) => ({ error: error.message }),
            },
          },
        },
      },
    },
    Remove: {
      initial: { target: 'Remove.Idle' },
      states: {
        Idle: {
          on: {
            RemoveProfile: {
              target: 'Remove.Working',
              update: 'root',
              data: ({ root, event }) => ({
                target: { name: event.name },
                update: { ...root, mutationSeq: root.mutationSeq + 1 },
              }),
            },
          },
        },
        Working: {
          invoke: {
            src: 'removeProfile',
            input: ({ state }) => state.name,
            onDone: {
              target: 'Remove.Idle',
              update: 'root',
              data: ({ root, output }) => ({ target: undefined, update: mergeReply(root, output) }),
            },
          },
        },
      },
    },
  },
})
