import { Api } from '@blob-1/client/api'
import { Webhook, WebhooksPayload } from '@blob-1/contracts'
import { Machine } from '@typeonce/effect-machine'
import { Array, Effect, Option, Schema } from 'effect'

import { Notify } from '~/lib/notify'

const ToggleJob = Schema.Struct({
  id: Schema.String,
  enabled: Schema.Boolean,
})
type ToggleJob = typeof ToggleJob.Type

const withToggled = (
  webhooks: ReadonlyArray<Webhook>,
  id: string,
  enabled: boolean,
): ReadonlyArray<Webhook> =>
  webhooks.map(webhook => (webhook.id === id ? { ...webhook, enabled } : webhook))

const replayToggles = (
  base: ReadonlyArray<Webhook>,
  jobs: ReadonlyArray<ToggleJob>,
): ReadonlyArray<Webhook> =>
  jobs.reduce((list, job) => withToggled(list, job.id, job.enabled), base)

/** Merges only the single toggled webhook row from the server reply into `current`. */
const mergeToggledRow = (
  current: ReadonlyArray<Webhook>,
  reply: ReadonlyArray<Webhook>,
  id: string,
): ReadonlyArray<Webhook> => {
  const updated = reply.find(item => item.id === id)
  if (updated === undefined) {
    return current.filter(item => item.id !== id)
  }
  return current.map(item => (item.id === id ? updated : item))
}

/** Merges newly created webhook rows without clobbering existing rows or removed ids. */
const mergeCreatedRows = (
  current: ReadonlyArray<Webhook>,
  reply: ReadonlyArray<Webhook>,
  removedIds: ReadonlyArray<string>,
): ReadonlyArray<Webhook> => {
  const existing = new Set(current.map(item => item.id))
  const removed = new Set(removedIds)
  const added = reply.filter(item => !existing.has(item.id) && !removed.has(item.id))
  return [...added, ...current]
}

const refreshWebhooks = Effect.gen(function* () {
  const api = yield* Api
  const notify = yield* Notify
  return yield* api
    .request('webhooks.list', {})
    .pipe(Effect.catch(error => notify.error(error.message).pipe(Effect.as(null))))
})

const setEnabled = Effect.fn('Webhooks.setEnabled')(function* (args: {
  readonly id: string
  readonly enabled: boolean
}) {
  const api = yield* Api
  const notify = yield* Notify
  return yield* api
    .request('webhooks.setEnabled', args)
    .pipe(Effect.catch(error => notify.error(error.message).pipe(Effect.as(null))))
})

const createWebhook = Effect.fn('Webhooks.create')(function* (name: string) {
  const api = yield* Api
  return yield* api.request('webhooks.create', { name })
})

const removeWebhook = Effect.fn('Webhooks.remove')(function* (id: string) {
  const api = yield* Api
  const notify = yield* Notify
  return yield* api
    .request('webhooks.remove', { id })
    .pipe(Effect.catch(error => notify.error(error.message).pipe(Effect.as(null))))
})

const ErrorMessage = Schema.NullOr(Schema.String)

export const WebhooksInput = WebhooksPayload
export type WebhooksInput = typeof WebhooksInput.Type

export const WebhooksStates = Machine.state({
  type: 'parallel',
  fields: {
    publicUrl: Schema.NullOr(Schema.String),
    serverList: Schema.Array(Webhook),
    webhooks: Schema.Array(Webhook),
    toggleJobs: Schema.Array(ToggleJob),
    removedIds: Schema.Array(Schema.String),
    inFlightWrites: Schema.Finite,
    mutationSeq: Schema.Finite,
    createSeq: Schema.Finite,
    /** A refresh was requested; `Refresh` runs it once no fetch or write is in flight. */
    refreshQueued: Schema.Boolean,
  },
  states: {
    Refresh: {
      states: {
        Idle: {},
        Working: { fields: { mutationSeq: Schema.Finite } },
      },
    },
    Toggle: {
      states: {
        Idle: {},
        Working: { fields: { id: Schema.String } },
      },
    },
    Create: {
      states: {
        Idle: { fields: { error: ErrorMessage } },
        Working: { fields: { name: Schema.String } },
      },
    },
    Remove: {
      states: {
        Idle: {},
        Working: { fields: { id: Schema.String } },
      },
    },
  },
})

export const WebhooksEvents = Machine.events({
  Refresh: {},
  Create: { name: Schema.String },
  ClearCreateError: {},
  SetEnabled: { id: Schema.String, enabled: Schema.Boolean },
  Remove: { id: Schema.String },
})

/** Manages webhook CRUD with optimistic toggling and row-scoped reply merges. */
export const WebhooksMachine = Machine.make({
  id: 'Webhooks',
  root: WebhooksStates,
  events: WebhooksEvents,
  input: WebhooksInput,
  effects: { refreshWebhooks, setEnabled, createWebhook, removeWebhook },
}).handle({
  root: ({ input }) => ({
    publicUrl: input.publicUrl,
    serverList: input.webhooks,
    webhooks: input.webhooks,
    toggleJobs: [],
    removedIds: [],
    inFlightWrites: 0,
    mutationSeq: 0,
    createSeq: 0,
    refreshQueued: false,
  }),
  on: {
    // Queued so a server push during a fetch or local write is retained and
    // replayed once the machine is idle.
    Refresh: {
      update: 'root',
      data: ({ root }) => ({ ...root, refreshQueued: true }),
    },
    SetEnabled: {
      update: 'root',
      data: ({ root, event }) => {
        const nextJobs = [
          ...root.toggleJobs.filter((job, index) => index === 0 || job.id !== event.id),
          { id: event.id, enabled: event.enabled },
        ]
        return {
          ...root,
          webhooks: withToggled(root.webhooks, event.id, event.enabled),
          toggleJobs: nextJobs,
          mutationSeq: root.mutationSeq + 1,
        }
      },
    },
  },
  states: {
    Refresh: {
      initial: { target: 'Refresh.Idle' },
      states: {
        Idle: {
          always: {
            guard: ({ root }) =>
              root.refreshQueued && root.inFlightWrites === 0 && root.toggleJobs.length === 0,
            target: 'Refresh.Working',
            update: 'root',
            data: ({ root }) => ({
              target: { mutationSeq: root.mutationSeq },
              update: { ...root, refreshQueued: false },
            }),
          },
        },
        Working: {
          invoke: {
            src: 'refreshWebhooks',
            onDone: {
              target: 'Refresh.Idle',
              update: 'root',
              data: ({ root, state, output }) => {
                if (output === null) {
                  return { target: undefined, update: root }
                }
                if (
                  state.mutationSeq !== root.mutationSeq ||
                  root.inFlightWrites > 0 ||
                  root.toggleJobs.length > 0
                ) {
                  // A local write superseded this snapshot; fetch again once writes finish.
                  return { target: undefined, update: { ...root, refreshQueued: true } }
                }
                return {
                  target: undefined,
                  update: {
                    ...root,
                    publicUrl: output.publicUrl,
                    serverList: output.webhooks,
                    webhooks: replayToggles(output.webhooks, root.toggleJobs),
                  },
                }
              },
            },
          },
        },
      },
    },
    Toggle: {
      initial: { target: 'Toggle.Idle' },
      states: {
        Idle: {
          always: {
            target: 'Toggle.Working',
            guard: ({ root }) => root.toggleJobs.length > 0,
            update: 'root',
            data: ({ root }) => {
              const job = Option.getOrThrow(Array.head(root.toggleJobs))
              return {
                target: { id: job.id },
                update: {
                  ...root,
                  inFlightWrites: root.inFlightWrites + 1,
                },
              }
            },
          },
        },
        Working: {
          invoke: {
            src: 'setEnabled',
            input: ({ root }) => Option.getOrThrow(Array.head(root.toggleJobs)),
            onDone: {
              target: 'Toggle.Idle',
              update: 'root',
              data: ({ root, state, output }) => {
                const rest = root.toggleJobs.slice(1)
                const nextInFlight = Math.max(0, root.inFlightWrites - 1)
                const base =
                  output === null
                    ? root.serverList
                    : mergeToggledRow(root.serverList, output.webhooks, state.id)
                return {
                  target: undefined,
                  update: {
                    ...root,
                    publicUrl: output === null ? root.publicUrl : output.publicUrl,
                    serverList: base,
                    webhooks: replayToggles(base, rest),
                    toggleJobs: rest,
                    inFlightWrites: nextInFlight,
                    removedIds: nextInFlight === 0 && rest.length === 0 ? [] : root.removedIds,
                    mutationSeq: root.mutationSeq + 1,
                  },
                }
              },
            },
          },
        },
      },
    },
    Create: {
      initial: { target: 'Create.Idle', data: { error: null } },
      states: {
        Idle: {
          on: {
            ClearCreateError: { update: 'Create.Idle', data: { error: null } },
            Create: {
              target: 'Create.Working',
              update: 'root',
              data: ({ root, event }) => ({
                target: { name: event.name },
                update: {
                  ...root,
                  inFlightWrites: root.inFlightWrites + 1,
                  mutationSeq: root.mutationSeq + 1,
                },
              }),
            },
          },
        },
        Working: {
          invoke: {
            src: 'createWebhook',
            input: ({ state }) => state.name,
            onDone: {
              target: 'Create.Idle',
              update: 'root',
              data: ({ root, output }) => {
                const nextInFlight = Math.max(0, root.inFlightWrites - 1)
                const base = mergeCreatedRows(root.serverList, output.webhooks, root.removedIds)
                return {
                  target: { error: null },
                  update: {
                    ...root,
                    publicUrl: output.publicUrl,
                    serverList: base,
                    webhooks: replayToggles(base, root.toggleJobs),
                    inFlightWrites: nextInFlight,
                    removedIds:
                      nextInFlight === 0 && root.toggleJobs.length === 0 ? [] : root.removedIds,
                    mutationSeq: root.mutationSeq + 1,
                    createSeq: root.createSeq + 1,
                  },
                }
              },
            },
            onFailure: {
              target: 'Create.Idle',
              update: 'root',
              data: ({ root, error }) => {
                const nextInFlight = Math.max(0, root.inFlightWrites - 1)
                return {
                  target: { error: error.message },
                  update: {
                    ...root,
                    inFlightWrites: nextInFlight,
                    removedIds:
                      nextInFlight === 0 && root.toggleJobs.length === 0 ? [] : root.removedIds,
                    mutationSeq: root.mutationSeq + 1,
                  },
                }
              },
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
            Remove: {
              target: 'Remove.Working',
              update: 'root',
              data: ({ root, event }) => ({
                target: { id: event.id },
                update: {
                  ...root,
                  inFlightWrites: root.inFlightWrites + 1,
                  mutationSeq: root.mutationSeq + 1,
                },
              }),
            },
          },
        },
        Working: {
          invoke: {
            src: 'removeWebhook',
            input: ({ state }) => state.id,
            onDone: {
              target: 'Remove.Idle',
              update: 'root',
              data: ({ root, state, output }) => {
                const nextInFlight = Math.max(0, root.inFlightWrites - 1)
                const base =
                  output === null
                    ? root.serverList
                    : root.serverList.filter(item => item.id !== state.id)
                const nextRemoved =
                  nextInFlight === 0 && root.toggleJobs.length === 0
                    ? []
                    : output === null
                      ? root.removedIds
                      : [...root.removedIds, state.id]
                const nextJobs =
                  output === null
                    ? root.toggleJobs
                    : root.toggleJobs.filter((job, index) => index === 0 || job.id !== state.id)
                return {
                  target: undefined,
                  update: {
                    ...root,
                    publicUrl: output === null ? root.publicUrl : output.publicUrl,
                    serverList: base,
                    webhooks: replayToggles(base, nextJobs),
                    toggleJobs: nextJobs,
                    inFlightWrites: nextInFlight,
                    removedIds: nextRemoved,
                    mutationSeq: root.mutationSeq + 1,
                  },
                }
              },
            },
          },
        },
      },
    },
  },
})
