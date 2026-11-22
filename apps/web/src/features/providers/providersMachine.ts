import { Api } from '@blob-1/client/api'
import { Provider, ProviderApi } from '@blob-1/contracts'
import { Machine } from '@typeonce/effect-machine'
import { Effect, Schema } from 'effect'

import { Notify } from '~/lib/notify'

const Draft = Schema.Struct({
  id: Schema.String,
  api: ProviderApi,
  baseUrl: Schema.String,
  apiKey: Schema.NullOr(Schema.String),
})
type Draft = typeof Draft.Type

const refreshProvider = Effect.fn('Providers.refresh')(function* (id: string) {
  const api = yield* Api
  const notify = yield* Notify
  return yield* api
    .request('providers.refresh', { id })
    .pipe(Effect.catch(error => notify.error(error.message).pipe(Effect.as(null))))
})

const removeProvider = Effect.fn('Providers.remove')(function* (id: string) {
  const api = yield* Api
  const notify = yield* Notify
  return yield* api
    .request('providers.remove', { id })
    .pipe(Effect.catch(error => notify.error(error.message).pipe(Effect.as(null))))
})

const addProvider = Effect.fn('Providers.add')(function* (draft: Draft) {
  const api = yield* Api
  return yield* api.request('providers.add', draft)
})

const setProviderKey = Effect.fn('Providers.setKey')(function* (input: {
  readonly id: string
  readonly apiKey: string | null
}) {
  const api = yield* Api
  return yield* api.request('providers.setKey', input)
})

export const ProvidersInput = Schema.Struct({ providers: Schema.Array(Provider) })
export type ProvidersInput = typeof ProvidersInput.Type

const ErrorMessage = Schema.NullOr(Schema.String)

export const ProvidersStates = Machine.state({
  type: 'parallel',
  fields: { providers: Schema.Array(Provider), refreshingId: Schema.NullOr(Schema.String) },
  states: {
    Action: {
      states: {
        Idle: {},
        Refreshing: { fields: { id: Schema.String } },
        Removing: { fields: { id: Schema.String } },
      },
    },
    Add: {
      states: {
        Closed: {},
        Editing: { fields: { error: ErrorMessage } },
        Submitting: { fields: { draft: Draft } },
      },
    },
    KeyDialog: {
      states: {
        Closed: {},
        Editing: { fields: { id: Schema.String, error: ErrorMessage } },
        Submitting: { fields: { id: Schema.String, apiKey: Schema.NullOr(Schema.String) } },
      },
    },
  },
})

export const ProvidersEvents = Machine.events({
  OpenAdd: {},
  CloseAdd: {},
  ClearAddError: {},
  SubmitAdd: {
    id: Schema.String,
    api: ProviderApi,
    baseUrl: Schema.String,
    apiKey: Schema.NullOr(Schema.String),
  },
  OpenKeyDialog: { id: Schema.String },
  CloseKeyDialog: {},
  ClearKeyError: {},
  SubmitKey: { id: Schema.String, apiKey: Schema.NullOr(Schema.String) },
  RefreshProvider: { id: Schema.String },
  RemoveProvider: { id: Schema.String },
})

/** The Providers page: the provider list plus refresh, key rotation, remove and add workflows. */
export const ProvidersMachine = Machine.make({
  id: 'Providers',
  root: ProvidersStates,
  events: ProvidersEvents,
  input: ProvidersInput,
  effects: { refreshProvider, removeProvider, addProvider, setProviderKey },
}).handle({
  root: ({ input }) => ({ providers: input.providers, refreshingId: null }),
  states: {
    Action: {
      initial: { target: 'Action.Idle' },
      states: {
        Idle: {
          on: {
            RefreshProvider: {
              target: 'Action.Refreshing',
              update: 'root',
              data: ({ event, root }) => ({
                target: { id: event.id },
                update: { ...root, refreshingId: event.id },
              }),
            },
            RemoveProvider: {
              target: 'Action.Removing',
              data: ({ event }) => ({ id: event.id }),
            },
          },
        },
        Refreshing: {
          invoke: {
            src: 'refreshProvider',
            input: ({ state }) => state.id,
            onDone: {
              target: 'Action.Idle',
              update: 'root',
              data: ({ root, output }) => ({
                target: undefined,
                update: { ...root, providers: output ?? root.providers, refreshingId: null },
              }),
            },
          },
        },
        Removing: {
          invoke: {
            src: 'removeProvider',
            input: ({ state }) => state.id,
            onDone: {
              target: 'Action.Idle',
              update: 'root',
              data: ({ root, output }) => ({
                target: undefined,
                update: { ...root, providers: output ?? root.providers },
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
              data: ({ event }) => ({
                draft: {
                  id: event.id,
                  api: event.api,
                  baseUrl: event.baseUrl,
                  apiKey: event.apiKey,
                },
              }),
            },
          },
        },
        Submitting: {
          invoke: {
            src: 'addProvider',
            input: ({ state }) => state.draft,
            onDone: {
              target: 'Add.Closed',
              update: 'root',
              data: ({ root, output }) => ({
                target: undefined,
                update: { ...root, providers: output },
              }),
            },
            onFailure: {
              target: 'Add.Editing',
              data: ({ error }) => ({ error: error.message }),
            },
          },
        },
      },
    },
    KeyDialog: {
      initial: { target: 'KeyDialog.Closed' },
      states: {
        Closed: {
          on: {
            OpenKeyDialog: {
              target: 'KeyDialog.Editing',
              data: ({ event }) => ({ id: event.id, error: null }),
            },
          },
        },
        Editing: {
          on: {
            CloseKeyDialog: { target: 'KeyDialog.Closed' },
            ClearKeyError: {
              update: 'KeyDialog.Editing',
              data: ({ state }) => ({ id: state.id, error: null }),
            },
            SubmitKey: {
              target: 'KeyDialog.Submitting',
              data: ({ event }) => ({ id: event.id, apiKey: event.apiKey }),
            },
          },
        },
        Submitting: {
          invoke: {
            src: 'setProviderKey',
            input: ({ state }) => ({ id: state.id, apiKey: state.apiKey }),
            onDone: {
              target: 'KeyDialog.Closed',
              update: 'root',
              data: ({ root, output }) => ({
                target: undefined,
                update: { ...root, providers: output },
              }),
            },
            onFailure: {
              target: 'KeyDialog.Editing',
              data: ({ state, error }) => ({ id: state.id, error: error.message }),
            },
          },
        },
      },
    },
  },
})
