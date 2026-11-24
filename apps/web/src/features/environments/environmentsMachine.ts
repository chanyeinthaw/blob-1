import { Api } from '@blob-1/client/api'
import { EnvironmentInfo, Environments, PendingNode, type RequestPayload } from '@blob-1/contracts'
import { Machine } from '@typeonce/effect-machine'
import { Effect, Schema } from 'effect'

import { Notify } from '~/lib/notify'

const fetchEnvironments = Effect.gen(function* () {
  const api = yield* Api
  return yield* api.request('environments.list', {}).pipe(Effect.orElseSucceed(() => null))
})

const acceptNode = Effect.fn('Environments.accept')(function* (args: {
  readonly nodeId: string
  readonly name: string
}) {
  const api = yield* Api
  const notify = yield* Notify
  const payload: RequestPayload<'environments.accept'> =
    args.name === '' ? { nodeId: args.nodeId } : { nodeId: args.nodeId, name: args.name }
  return yield* api
    .request('environments.accept', payload)
    .pipe(Effect.catch(error => notify.error(error.message).pipe(Effect.as(null))))
})

const removeEnvironment = Effect.fn('Environments.remove')(function* (name: string) {
  const api = yield* Api
  const notify = yield* Notify
  return yield* api
    .request('environments.remove', { name })
    .pipe(Effect.catch(error => notify.error(error.message).pipe(Effect.as(null))))
})

const renameEnvironment = Effect.fn('Environments.rename')(function* (args: {
  readonly name: string
  readonly to: string
}) {
  const api = yield* Api
  const notify = yield* Notify
  return yield* api
    .request('environments.rename', args)
    .pipe(Effect.catch(error => notify.error(error.message).pipe(Effect.as(null))))
})

export const EnvironmentsInput = Environments
export type EnvironmentsInput = typeof EnvironmentsInput.Type

export const EnvironmentsStates = Machine.state({
  type: 'parallel',
  fields: {
    pending: Schema.Array(PendingNode),
    environments: Schema.Array(EnvironmentInfo),
    mutationSeq: Schema.Finite,
  },
  states: {
    Sync: {
      states: {
        Idle: {},
        Refreshing: { fields: { mutationSeq: Schema.Finite } },
      },
    },
    Action: {
      states: {
        Idle: {},
        Accepting: { fields: { nodeId: Schema.String, name: Schema.String } },
        Removing: { fields: { name: Schema.String } },
        Renaming: { fields: { name: Schema.String, to: Schema.String } },
      },
    },
  },
})

export const EnvironmentsEvents = Machine.events({
  Refresh: {},
  Accept: { nodeId: Schema.String, name: Schema.String },
  Remove: { name: Schema.String },
  Rename: { name: Schema.String, to: Schema.String },
})

/** Merges a whole-list reply into root; `null` (a notified failure) keeps the current list. */
const mergeReply = (
  root: {
    readonly pending: ReadonlyArray<PendingNode>
    readonly environments: ReadonlyArray<EnvironmentInfo>
    readonly mutationSeq: number
  },
  output: Environments | null,
) => ({
  ...root,
  pending: output?.pending ?? root.pending,
  environments: output?.environments ?? root.environments,
  mutationSeq: root.mutationSeq + 1,
})

/** Manages the environments page: pending-node accepts plus environment rename and revoke. */
export const EnvironmentsMachine = Machine.make({
  id: 'Environments',
  root: EnvironmentsStates,
  events: EnvironmentsEvents,
  input: EnvironmentsInput,
  effects: { fetchEnvironments, acceptNode, removeEnvironment, renameEnvironment },
}).handle({
  root: ({ input }) => ({
    pending: input.pending,
    environments: input.environments,
    mutationSeq: 0,
  }),
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
            src: 'fetchEnvironments',
            onDone: {
              target: 'Sync.Idle',
              update: 'root',
              data: ({ root, state, output }) => ({
                target: undefined,
                update:
                  output === null || state.mutationSeq !== root.mutationSeq
                    ? root
                    : {
                        ...root,
                        pending: output.pending,
                        environments: output.environments,
                      },
              }),
            },
          },
        },
      },
    },
    Action: {
      initial: { target: 'Action.Idle' },
      states: {
        Idle: {
          on: {
            Accept: {
              target: 'Action.Accepting',
              update: 'root',
              data: ({ root, event }) => ({
                target: { nodeId: event.nodeId, name: event.name },
                update: { ...root, mutationSeq: root.mutationSeq + 1 },
              }),
            },
            Remove: {
              target: 'Action.Removing',
              update: 'root',
              data: ({ root, event }) => ({
                target: { name: event.name },
                update: { ...root, mutationSeq: root.mutationSeq + 1 },
              }),
            },
            Rename: {
              target: 'Action.Renaming',
              update: 'root',
              data: ({ root, event }) => ({
                target: { name: event.name, to: event.to },
                update: { ...root, mutationSeq: root.mutationSeq + 1 },
              }),
            },
          },
        },
        Accepting: {
          invoke: {
            src: 'acceptNode',
            input: ({ state }) => ({ nodeId: state.nodeId, name: state.name }),
            onDone: {
              target: 'Action.Idle',
              update: 'root',
              data: ({ root, output }) => ({ target: undefined, update: mergeReply(root, output) }),
            },
          },
        },
        Removing: {
          invoke: {
            src: 'removeEnvironment',
            input: ({ state }) => state.name,
            onDone: {
              target: 'Action.Idle',
              update: 'root',
              data: ({ root, output }) => ({ target: undefined, update: mergeReply(root, output) }),
            },
          },
        },
        Renaming: {
          invoke: {
            src: 'renameEnvironment',
            input: ({ state }) => ({ name: state.name, to: state.to }),
            onDone: {
              target: 'Action.Idle',
              update: 'root',
              data: ({ root, output }) => ({ target: undefined, update: mergeReply(root, output) }),
            },
          },
        },
      },
    },
  },
})
