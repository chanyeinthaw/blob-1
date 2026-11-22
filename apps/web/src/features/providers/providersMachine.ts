import { Api } from '@blob-1/client/api'
import { Provider, ProviderApi } from '@blob-1/contracts'
import { Machine } from '@typeonce/effect-machine'
import { Effect, Schema } from 'effect'

import { Notify } from '~/lib/notify'

const Draft = Schema.Struct({
  id: Schema.String,
  api: ProviderApi,
  baseUrl: Schema.String,
  keyEnv: Schema.String,
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

export const ProvidersInput = Schema.Struct({ providers: Schema.Array(Provider) })
export type ProvidersInput = typeof ProvidersInput.Type

const ErrorMessage = Schema.NullOr(Schema.String)

export const ProvidersStates = Machine.state({
  type: 'parallel',
  fields: { providers: Schema.Array(Provider), refreshingId: Schema.NullOr(Schema.String) },
  states: {
    Refresh: {
      states: {
        Idle: {},
        Working: { fields: { id: Schema.String } },
      },
    },
    Remove: {
      states: {
        Idle: {},
        Working: { fields: { id: Schema.String } },
      },
    },
    Add: {
      states: {
        Closed: {},
        Editing: { fields: { error: ErrorMessage } },
        Submitting: { fields: { draft: Draft } },
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
    keyEnv: Schema.String,
  },
  RefreshProvider: { id: Schema.String },
  RemoveProvider: { id: Schema.String },
})

/** The Providers page: the provider list plus refresh, remove and add workflows. */
export const ProvidersMachine = Machine.make({
  id: 'Providers',
  root: ProvidersStates,
  events: ProvidersEvents,
  input: ProvidersInput,
  effects: { refreshProvider, removeProvider, addProvider },
}).handle({
  root: ({ input }) => ({ providers: input.providers, refreshingId: null }),
  states: {
    Refresh: {
      initial: { target: 'Refresh.Idle' },
      states: {
        Idle: {
          on: {
            RefreshProvider: {
              target: 'Refresh.Working',
              update: 'root',
              data: ({ event, root }) => ({
                target: { id: event.id },
                update: { ...root, refreshingId: event.id },
              }),
            },
          },
        },
        Working: {
          invoke: {
            src: 'refreshProvider',
            input: ({ state }) => state.id,
            onDone: {
              target: 'Refresh.Idle',
              update: 'root',
              data: ({ root, output }) => ({
                target: undefined,
                update: { ...root, providers: output ?? root.providers, refreshingId: null },
              }),
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
            RemoveProvider: {
              target: 'Remove.Working',
              data: ({ event }) => ({ id: event.id }),
            },
          },
        },
        Working: {
          invoke: {
            src: 'removeProvider',
            input: ({ state }) => state.id,
            onDone: {
              target: 'Remove.Idle',
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
                  keyEnv: event.keyEnv,
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
  },
})
