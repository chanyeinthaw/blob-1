import { Api } from '@blob-1/client/api'
import { Secret, SecretRequestLink, SecretsPayload } from '@blob-1/contracts'
import { Machine } from '@typeonce/effect-machine'
import { Effect, Schema } from 'effect'

import { Notify } from '~/lib/notify'

const SecretMutation = Schema.Union([
  Schema.Struct({ kind: Schema.Literal('set'), name: Schema.String, value: Schema.String }),
  Schema.Struct({ kind: Schema.Literal('remove'), name: Schema.String }),
])
type SecretMutation = typeof SecretMutation.Type

const refreshSecrets = Effect.gen(function* () {
  const api = yield* Api
  const notify = yield* Notify
  return yield* api
    .request('secrets.list', {})
    .pipe(Effect.catch(error => notify.error(error.message).pipe(Effect.as(null))))
})

const mutateSecret = Effect.fn('Secrets.mutate')(function* (operation: SecretMutation) {
  const api = yield* Api
  if (operation.kind === 'set') {
    return yield* api.request('secrets.set', { name: operation.name, value: operation.value })
  }
  return yield* api.request('secrets.remove', { name: operation.name })
})

const requestSecret = Effect.fn('Secrets.request')(function* (args: {
  readonly name: string
  readonly label: string | null
}) {
  const api = yield* Api
  return yield* api.request('secrets.request', args)
})

export const SecretsInput = SecretsPayload
export type SecretsInput = typeof SecretsInput.Type

export const SecretsStates = Machine.state({
  type: 'parallel',
  fields: {
    publicUrl: Schema.NullOr(Schema.String),
    secrets: Schema.Array(Secret),
    refreshQueued: Schema.Boolean,
    inFlightWrites: Schema.Finite,
    mutationSeq: Schema.Finite,
    saveSeq: Schema.Finite,
    mutationError: Schema.NullOr(Schema.String),
    mutationErrorKind: Schema.NullOr(Schema.Literals(['set', 'remove'])),
    requestLink: Schema.NullOr(SecretRequestLink),
    requestError: Schema.NullOr(Schema.String),
  },
  states: {
    Refresh: {
      states: {
        Idle: {},
        Working: { fields: { mutationSeq: Schema.Finite } },
      },
    },
    Mutation: {
      states: {
        Idle: {},
        Working: { fields: { operation: SecretMutation } },
      },
    },
    Request: {
      states: {
        Idle: {},
        Working: { fields: { name: Schema.String, label: Schema.NullOr(Schema.String) } },
      },
    },
  },
})

export const SecretsEvents = Machine.events({
  Refresh: {},
  SetSecret: { name: Schema.String, value: Schema.String },
  RemoveSecret: { name: Schema.String },
  ReplaceData: { state: SecretsPayload },
  ClearMutationError: {},
  RequestSecret: { name: Schema.String, label: Schema.NullOr(Schema.String) },
  ClearRequestError: {},
})

/** Keeps secret writes short-lived and queues refreshes around in-flight writes. */
export const SecretsMachine = Machine.make({
  id: 'Secrets',
  root: SecretsStates,
  events: SecretsEvents,
  input: SecretsInput,
  effects: { refreshSecrets, mutateSecret, requestSecret },
}).handle({
  root: ({ input }) => ({
    publicUrl: input.publicUrl,
    secrets: input.secrets,
    refreshQueued: false,
    inFlightWrites: 0,
    mutationSeq: 0,
    saveSeq: 0,
    mutationError: null,
    mutationErrorKind: null,
    requestLink: null,
    requestError: null,
  }),
  on: {
    Refresh: {
      update: 'root',
      data: ({ root }) => ({ ...root, refreshQueued: true }),
    },
    ReplaceData: {
      update: 'root',
      data: ({ root, event }) => ({
        ...root,
        publicUrl: event.state.publicUrl,
        secrets: event.state.secrets,
        mutationSeq: root.mutationSeq + 1,
      }),
    },
    ClearMutationError: {
      update: 'root',
      data: ({ root }) => ({ ...root, mutationError: null, mutationErrorKind: null }),
    },
    ClearRequestError: {
      update: 'root',
      data: ({ root }) => ({ ...root, requestError: null }),
    },
  },
  states: {
    Refresh: {
      initial: { target: 'Refresh.Idle' },
      states: {
        Idle: {
          always: {
            guard: ({ root }) => root.refreshQueued && root.inFlightWrites === 0,
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
            src: 'refreshSecrets',
            onDone: {
              target: 'Refresh.Idle',
              update: 'root',
              data: ({ root, state, output }) => {
                if (output === null) {
                  return { target: undefined, update: root }
                }
                if (state.mutationSeq !== root.mutationSeq || root.inFlightWrites > 0) {
                  return { target: undefined, update: { ...root, refreshQueued: true } }
                }
                return {
                  target: undefined,
                  update: { ...root, publicUrl: output.publicUrl, secrets: output.secrets },
                }
              },
            },
          },
        },
      },
    },
    Mutation: {
      initial: { target: 'Mutation.Idle' },
      states: {
        Idle: {
          on: {
            SetSecret: {
              target: 'Mutation.Working',
              update: 'root',
              data: ({ root, event }) => ({
                target: { operation: { kind: 'set', name: event.name, value: event.value } },
                update: {
                  ...root,
                  inFlightWrites: root.inFlightWrites + 1,
                  mutationSeq: root.mutationSeq + 1,
                  mutationError: null,
                  mutationErrorKind: null,
                },
              }),
            },
            RemoveSecret: {
              target: 'Mutation.Working',
              update: 'root',
              data: ({ root, event }) => ({
                target: { operation: { kind: 'remove', name: event.name } },
                update: {
                  ...root,
                  inFlightWrites: root.inFlightWrites + 1,
                  mutationSeq: root.mutationSeq + 1,
                  mutationError: null,
                  mutationErrorKind: null,
                },
              }),
            },
          },
        },
        Working: {
          invoke: {
            src: 'mutateSecret',
            input: ({ state }) => state.operation,
            onDone: {
              target: 'Mutation.Idle',
              update: 'root',
              data: ({ root, state, output }) => ({
                target: undefined,
                update: {
                  ...root,
                  publicUrl: output.publicUrl,
                  secrets: output.secrets,
                  inFlightWrites: Math.max(0, root.inFlightWrites - 1),
                  mutationSeq: root.mutationSeq + 1,
                  saveSeq: state.operation.kind === 'set' ? root.saveSeq + 1 : root.saveSeq,
                  mutationError: null,
                  mutationErrorKind: null,
                },
              }),
            },
            onFailure: {
              target: 'Mutation.Idle',
              update: 'root',
              data: ({ root, state, error }) => ({
                target: undefined,
                update: {
                  ...root,
                  inFlightWrites: Math.max(0, root.inFlightWrites - 1),
                  mutationSeq: root.mutationSeq + 1,
                  mutationError: error.message,
                  mutationErrorKind: state.operation.kind,
                },
              }),
            },
          },
        },
      },
    },
    Request: {
      initial: { target: 'Request.Idle' },
      states: {
        Idle: {
          on: {
            RequestSecret: {
              target: 'Request.Working',
              update: 'root',
              data: ({ root, event }) => ({
                target: { name: event.name, label: event.label },
                update: { ...root, requestLink: null, requestError: null },
              }),
            },
          },
        },
        Working: {
          invoke: {
            src: 'requestSecret',
            input: ({ state }) => ({ name: state.name, label: state.label }),
            onDone: {
              target: 'Request.Idle',
              update: 'root',
              data: ({ root, output }) => ({
                target: undefined,
                update: { ...root, requestLink: output, requestError: null },
              }),
            },
            onFailure: {
              target: 'Request.Idle',
              update: 'root',
              data: ({ root, error }) => ({
                target: undefined,
                update: { ...root, requestError: error.message },
              }),
            },
          },
        },
      },
    },
  },
})
