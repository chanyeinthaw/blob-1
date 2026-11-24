import { Api } from '@blob-1/client/api'
import { BotSummary } from '@blob-1/contracts'
import { Machine } from '@typeonce/effect-machine'
import { Effect, Schema } from 'effect'

import { Notify } from '~/lib/notify'

const fetchBots = Effect.gen(function* () {
  const api = yield* Api
  return yield* api.request('bots.list', {}).pipe(Effect.orElseSucceed(() => null))
})

const RosterOp = Schema.Union([
  Schema.Struct({ kind: Schema.Literal('create'), id: Schema.String, name: Schema.String }),
  Schema.Struct({
    kind: Schema.Literal('update'),
    id: Schema.String,
    name: Schema.NullOr(Schema.String),
    enabled: Schema.NullOr(Schema.Boolean),
    botGeneration: Schema.Finite,
  }),
  Schema.Struct({
    kind: Schema.Literal('remove'),
    id: Schema.String,
    botGeneration: Schema.Finite,
  }),
  Schema.Struct({
    kind: Schema.Literal('setEnvironment'),
    id: Schema.String,
    environment: Schema.String,
    botGeneration: Schema.Finite,
  }),
])
type RosterOp = typeof RosterOp.Type

const MutateOk = Schema.Struct({ ok: Schema.Literal(true), bots: Schema.Array(BotSummary) })
const MutateFailed = Schema.Struct({ ok: Schema.Literal(false), error: Schema.String })
type MutateResult = typeof MutateOk.Type | typeof MutateFailed.Type

/** Runs one roster write at a time so whole-roster replies can never race each other. */
const mutateBot = Effect.fn('Bots.mutate')(function* (op: RosterOp) {
  const api = yield* Api
  const notify = yield* Notify
  const write = (() => {
    switch (op.kind) {
      case 'create':
        return api.request('bots.create', { id: op.id, name: op.name })
      case 'update':
        return api.request('bots.update', {
          id: op.id,
          name: op.name,
          enabled: op.enabled,
          botGeneration: op.botGeneration,
        })
      case 'remove':
        return api.request('bots.remove', { id: op.id, botGeneration: op.botGeneration })
      case 'setEnvironment':
        return api.request('bots.setEnvironment', {
          botId: op.id,
          environment: op.environment,
          botGeneration: op.botGeneration,
        })
    }
  })()
  return yield* write.pipe(
    Effect.map((bots): MutateResult => ({ ok: true, bots })),
    Effect.catch(error =>
      op.kind === 'create'
        ? Effect.succeed<MutateResult>({ ok: false, error: error.message })
        : notify
            .error(error.message)
            .pipe(Effect.as<MutateResult>({ ok: false, error: error.message })),
    ),
  )
})

export const BotsInput = Schema.Array(BotSummary)
export type BotsInput = typeof BotsInput.Type

export const BotsStates = Machine.state({
  type: 'parallel',
  fields: {
    bots: Schema.Array(BotSummary),
    createError: Schema.NullOr(Schema.String),
    lastCreatedId: Schema.NullOr(Schema.String),
    lastRemovedId: Schema.NullOr(Schema.String),
    busyIds: Schema.Array(Schema.String),
    pendingOps: Schema.Array(RosterOp),
    mutationSeq: Schema.Finite,
  },
  states: {
    Mutate: {
      states: {
        Idle: {},
        Processing: { fields: { op: RosterOp } },
      },
    },
    Sync: {
      states: {
        Idle: {},
        Refreshing: { fields: { mutationSeq: Schema.Finite } },
      },
    },
  },
})

export const BotsEvents = Machine.events({
  Create: { id: Schema.String, name: Schema.String },
  ClearCreateState: {},
  ClearRemovedState: {},
  Update: {
    id: Schema.String,
    name: Schema.NullOr(Schema.String),
    enabled: Schema.NullOr(Schema.Boolean),
    botGeneration: Schema.Finite,
  },
  Remove: { id: Schema.String, botGeneration: Schema.Finite },
  SetEnvironment: {
    id: Schema.String,
    environment: Schema.String,
    botGeneration: Schema.Finite,
  },
  Refresh: {},
})

interface Root {
  readonly bots: ReadonlyArray<BotSummary>
  readonly createError: string | null
  readonly lastCreatedId: string | null
  readonly lastRemovedId: string | null
  readonly busyIds: ReadonlyArray<string>
  readonly pendingOps: ReadonlyArray<RosterOp>
  readonly mutationSeq: number
}

const enqueue = (root: Root, op: RosterOp): Root => ({
  ...root,
  pendingOps: [...root.pendingOps, op],
  busyIds: root.busyIds.includes(op.id) ? root.busyIds : [...root.busyIds, op.id],
  createError: op.kind === 'create' ? null : root.createError,
  lastCreatedId: op.kind === 'create' ? null : root.lastCreatedId,
  lastRemovedId: op.kind === 'remove' ? null : root.lastRemovedId,
  mutationSeq: root.mutationSeq + 1,
})

/** Pops the queue head; only reachable when the `always` guard saw a non-empty queue. */
const dequeue = (ops: ReadonlyArray<RosterOp>): [RosterOp, ReadonlyArray<RosterOp>] => {
  const [op, ...rest] = ops
  if (op === undefined) throw new Error('dequeue on empty roster queue')
  return [op, rest]
}

/** Manages the bot roster (create, rename, pause/resume, delete, and live background refresh). */
export const BotsMachine = Machine.make({
  id: 'Bots',
  root: BotsStates,
  events: BotsEvents,
  input: BotsInput,
  effects: { fetchBots, mutateBot },
}).handle({
  root: ({ input }) => ({
    bots: input,
    createError: null,
    lastCreatedId: null,
    lastRemovedId: null,
    busyIds: [],
    pendingOps: [],
    mutationSeq: 0,
  }),
  states: {
    Mutate: {
      initial: { target: 'Mutate.Idle' },
      states: {
        Idle: {
          always: {
            guard: ({ root }) => root.pendingOps.length > 0,
            target: 'Mutate.Processing',
            update: 'root',
            data: ({ root }) => {
              const [op, pendingOps] = dequeue(root.pendingOps)
              return { target: { op }, update: { ...root, pendingOps } }
            },
          },
          on: {
            Create: {
              update: 'root',
              data: ({ root, event }) =>
                enqueue(root, { kind: 'create', id: event.id, name: event.name }),
            },
            Update: {
              update: 'root',
              data: ({ root, event }) =>
                enqueue(root, {
                  kind: 'update',
                  id: event.id,
                  name: event.name,
                  enabled: event.enabled,
                  botGeneration: event.botGeneration,
                }),
            },
            Remove: {
              update: 'root',
              data: ({ root, event }) =>
                enqueue(root, {
                  kind: 'remove',
                  id: event.id,
                  botGeneration: event.botGeneration,
                }),
            },
            SetEnvironment: {
              update: 'root',
              data: ({ root, event }) =>
                enqueue(root, {
                  kind: 'setEnvironment',
                  id: event.id,
                  environment: event.environment,
                  botGeneration: event.botGeneration,
                }),
            },
            ClearCreateState: {
              update: 'root',
              data: ({ root }) => ({ ...root, createError: null, lastCreatedId: null }),
            },
            ClearRemovedState: {
              update: 'root',
              data: ({ root }) => ({ ...root, lastRemovedId: null }),
            },
          },
        },
        Processing: {
          invoke: {
            src: 'mutateBot',
            input: ({ state }) => state.op,
            onDone: {
              target: 'Mutate.Idle',
              update: 'root',
              data: ({ root, state, output }) => {
                // Keep the id busy while later queued ops for it still exist.
                const busyIds = root.pendingOps.some(op => op.id === state.op.id)
                  ? root.busyIds
                  : root.busyIds.filter(id => id !== state.op.id)
                if (!output.ok) {
                  return {
                    target: undefined,
                    update: {
                      ...root,
                      busyIds,
                      createError: state.op.kind === 'create' ? output.error : root.createError,
                      lastCreatedId: state.op.kind === 'create' ? null : root.lastCreatedId,
                    },
                  }
                }
                return {
                  target: undefined,
                  update: {
                    ...root,
                    busyIds,
                    bots: output.bots,
                    lastCreatedId: state.op.kind === 'create' ? state.op.id : root.lastCreatedId,
                    lastRemovedId: state.op.kind === 'remove' ? state.op.id : root.lastRemovedId,
                    createError: state.op.kind === 'create' ? null : root.createError,
                    mutationSeq: root.mutationSeq + 1,
                  },
                }
              },
            },
          },
          on: {
            Create: {
              update: 'root',
              data: ({ root, event }) =>
                enqueue(root, { kind: 'create', id: event.id, name: event.name }),
            },
            Update: {
              update: 'root',
              data: ({ root, event }) =>
                enqueue(root, {
                  kind: 'update',
                  id: event.id,
                  name: event.name,
                  enabled: event.enabled,
                  botGeneration: event.botGeneration,
                }),
            },
            Remove: {
              update: 'root',
              data: ({ root, event }) =>
                enqueue(root, {
                  kind: 'remove',
                  id: event.id,
                  botGeneration: event.botGeneration,
                }),
            },
            SetEnvironment: {
              update: 'root',
              data: ({ root, event }) =>
                enqueue(root, {
                  kind: 'setEnvironment',
                  id: event.id,
                  environment: event.environment,
                  botGeneration: event.botGeneration,
                }),
            },
            ClearCreateState: {
              update: 'root',
              data: ({ root }) => ({ ...root, createError: null, lastCreatedId: null }),
            },
            ClearRemovedState: {
              update: 'root',
              data: ({ root }) => ({ ...root, lastRemovedId: null }),
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
              target: 'Sync.Refreshing',
              data: ({ root }) => ({ mutationSeq: root.mutationSeq }),
            },
          },
        },
        Refreshing: {
          invoke: {
            src: 'fetchBots',
            onDone: {
              target: 'Sync.Idle',
              update: 'root',
              data: ({ root, state, output }) => ({
                target: undefined,
                update:
                  output !== null && state.mutationSeq === root.mutationSeq
                    ? { ...root, bots: output }
                    : root,
              }),
            },
          },
        },
      },
    },
  },
})
