import { Api } from '@blob-1/client/api'
import { Models, type ModelInfo, Slot } from '@blob-1/contracts'
import { Machine } from '@typeonce/effect-machine'
import { Array, Effect, Option, Schema } from 'effect'

import { Notify } from '~/lib/notify'

const ImageJob = Schema.Struct({
  provider: Schema.String,
  modelId: Schema.String,
  images: Schema.Boolean,
})
type ImageJob = typeof ImageJob.Type

const ContextJob = Schema.Struct({
  provider: Schema.String,
  modelId: Schema.String,
  limit: Schema.NullOr(Schema.Finite),
  version: Schema.Finite,
})
type ContextJob = typeof ContextJob.Type

const ContextDraft = Schema.Struct({
  value: Schema.String,
  version: Schema.Finite,
  status: Schema.String,
})
export type ContextDraft = typeof ContextDraft.Type

const rootFields = {
  server: Models,
  shown: Models,
  roleStatuses: Schema.Record(Schema.String, Schema.String),
  contextDrafts: Schema.Record(Schema.String, ContextDraft),
  imageJobs: Schema.Array(ImageJob),
  contextJobs: Schema.Array(ContextJob),
  /** Bumped on every confirmed Compaction save, so an older Main reply can't overwrite it. */
  compactionVersion: Schema.Finite,
}
const RootData = Schema.Struct(rootFields)
type RootData = typeof RootData.Type

export const contextKey = (provider: string, modelId: string): string =>
  JSON.stringify([provider, modelId])

const keyOf = (job: ContextJob): string => contextKey(job.provider, job.modelId)

/** Parses a context-override draft: undefined = invalid, null = clear the override. */
export const parseContextLimit = (input: string): number | null | undefined => {
  const value = input.trim()
  if (value === '') {
    return null
  }
  if (!/^(?:\d+|\d{1,3}(?:,\d{3})+)$/.test(value)) {
    return undefined
  }
  const limit = Number(value.replace(/,/g, ''))
  return Number.isSafeInteger(limit) && limit > 0 ? limit : undefined
}

export const contextText = (context: number | null): string =>
  context === null ? '—' : context.toLocaleString('en-US')

const sameSlot = (a: Slot | null, b: Slot | null): boolean =>
  JSON.stringify(a) === JSON.stringify(b)

const findInfo = (models: Models, provider: string, modelId: string): ModelInfo | undefined =>
  models.providers.find(entry => entry.id === provider)?.models.find(entry => entry.id === modelId)

const withModelInfo = (
  models: Models,
  provider: string,
  modelId: string,
  update: (info: ModelInfo) => ModelInfo,
): Models => ({
  ...models,
  providers: models.providers.map(entry =>
    entry.id === provider
      ? {
          ...entry,
          models: entry.models.map(info => (info.id === modelId ? update(info) : info)),
        }
      : entry,
  ),
})

const withImages = (models: Models, provider: string, modelId: string, images: boolean): Models =>
  withModelInfo(models, provider, modelId, info => ({ ...info, images }))

const withLimit = (
  models: Models,
  provider: string,
  modelId: string,
  limit: number | null,
): Models => withModelInfo(models, provider, modelId, info => ({ ...info, limit }))

type Role = 'main' | 'compaction' | 'utility'

/** A role reply owns that role's slot; preserves newer optimistic edits if shown moved on. */
const confirmRole = <R extends Role>(
  root: Pick<RootData, 'server' | 'shown'>,
  role: R,
  output: Models,
  submitted: Models[R],
): Pick<RootData, 'server' | 'shown'> => ({
  server: { ...root.server, [role]: output[role] },
  shown: {
    ...root.shown,
    [role]: sameSlot(root.shown[role], submitted) ? output[role] : root.shown[role],
  },
})

/**
 * A Main reply also carries Compaction's effective reasoning, which inherits from Main when
 * Compaction has no override. It confirms Compaction only if no Compaction reply landed since
 * the Main save started, and leaves a Compaction edit in flight on screen.
 */
const confirmMain = (
  root: RootData,
  output: Models,
  submitted: Slot,
  compactionVersion: number,
): Pick<RootData, 'server' | 'shown'> => {
  const own = confirmRole(root, 'main', output, submitted)
  if (compactionVersion !== root.compactionVersion) return own
  const compactionPending =
    root.roleStatuses['compaction'] === 'Saving…' ||
    !sameSlot(root.shown.compaction, root.server.compaction)
  return {
    server: { ...own.server, compaction: output.compaction },
    shown: compactionPending ? own.shown : { ...own.shown, compaction: output.compaction },
  }
}

/** Settles the head image job, replaying queued toggles over the confirmed flag. */
const settleImages = (
  root: RootData,
  output: Models | null,
): Pick<RootData, 'server' | 'shown' | 'imageJobs'> => {
  const job = root.imageJobs[0]
  if (job === undefined) {
    return { server: root.server, shown: root.shown, imageJobs: [] }
  }
  const confirmed = findInfo(output ?? root.server, job.provider, job.modelId)?.images ?? job.images
  const rest = root.imageJobs.slice(1)
  const base = (models: Models) => withImages(models, job.provider, job.modelId, confirmed)
  return {
    server: output === null ? root.server : base(root.server),
    shown: rest.reduce(
      (models, queued) => withImages(models, queued.provider, queued.modelId, queued.images),
      base(root.shown),
    ),
    imageJobs: rest,
  }
}

const withDraftStatus = (root: RootData, key: string, status: string): RootData => {
  const draft = root.contextDrafts[key]
  return draft === undefined
    ? root
    : {
        ...root,
        contextDrafts: { ...root.contextDrafts, [key]: { ...draft, status } },
      }
}

const enqueueContextJob = (root: RootData, job: ContextJob): ReadonlyArray<ContextJob> => [
  ...root.contextJobs.filter((entry, index) => index === 0 || keyOf(entry) !== keyOf(job)),
  job,
]

/** Validates a context draft on blur/Enter and queues a save, deduped by model key. */
const flushContextDraft = (
  event: { readonly provider: string; readonly modelId: string },
  root: RootData,
): RootData => {
  const key = contextKey(event.provider, event.modelId)
  const draft = root.contextDrafts[key]
  if (draft === undefined) {
    return root
  }
  const limit = parseContextLimit(draft.value)
  if (limit === undefined) {
    return withDraftStatus(root, key, 'Positive whole number')
  }
  const job: ContextJob = {
    provider: event.provider,
    modelId: event.modelId,
    limit,
    version: draft.version,
  }
  const pending = root.contextJobs.some(entry => keyOf(entry) === key)
  if (pending) {
    return {
      ...root,
      contextJobs: enqueueContextJob(root, job),
      contextDrafts: { ...root.contextDrafts, [key]: { ...draft, status: 'Saving…' } },
    }
  }
  const info = findInfo(root.server, event.provider, event.modelId)
  if (info === undefined) {
    return root
  }
  if (info.limit === limit) {
    return withDraftStatus(root, key, '')
  }
  return {
    ...root,
    contextJobs: enqueueContextJob(root, job),
    contextDrafts: { ...root.contextDrafts, [key]: { ...draft, status: 'Saving…' } },
  }
}

/** A context reply owns only that model's limit; other confirmed state is untouched. */
const confirmLimit = (
  root: RootData,
  job: ContextJob,
  models: Models,
): Pick<RootData, 'server' | 'shown'> => {
  const confirmed = findInfo(models, job.provider, job.modelId)?.limit ?? job.limit
  return {
    server: withLimit(root.server, job.provider, job.modelId, confirmed),
    shown: withLimit(root.shown, job.provider, job.modelId, confirmed),
  }
}

/** Settles the head context job: `fallback` is the status when the draft never moved on. */
const settleContext = (root: RootData, models: Models | null, fallback: string): RootData => {
  const job = root.contextJobs[0]
  if (job === undefined) {
    return root
  }
  const key = keyOf(job)
  const rest = root.contextJobs.slice(1)
  const draft = root.contextDrafts[key]
  const queued = rest.find(entry => keyOf(entry) === key)
  const status =
    draft === undefined
      ? ''
      : draft.version === (queued?.version ?? job.version)
        ? queued === undefined
          ? fallback
          : 'Saving…'
        : draft.status === 'Saving…'
          ? ''
          : draft.status
  const merged =
    models === null ? { server: root.server, shown: root.shown } : confirmLimit(root, job, models)
  return {
    ...root,
    ...merged,
    contextJobs: rest,
    contextDrafts:
      draft === undefined
        ? root.contextDrafts
        : { ...root.contextDrafts, [key]: { ...draft, status } },
  }
}

const notify = (message: string) => Effect.flatMap(Notify, service => service.error(message))

const fetchModels = Effect.gen(function* () {
  const api = yield* Api
  return yield* api.request('models.get', {}).pipe(Effect.orElseSucceed(() => null))
})

const setMain = Effect.fn('ModelsSettings.setMain')(function* (slot: Slot) {
  const api = yield* Api
  return yield* api
    .request('models.setMain', slot)
    .pipe(Effect.tapError(error => notify(error.message)))
})

const setCompaction = Effect.fn('ModelsSettings.setCompaction')(function* (slot: Slot | null) {
  const api = yield* Api
  return yield* api
    .request('models.setCompaction', { slot })
    .pipe(Effect.tapError(error => notify(error.message)))
})

const setUtility = Effect.fn('ModelsSettings.setUtility')(function* (slot: Slot | null) {
  const api = yield* Api
  return yield* api
    .request('models.setUtility', { slot })
    .pipe(Effect.tapError(error => notify(error.message)))
})

const setImages = Effect.fn('ModelsSettings.setImages')(function* (job: ImageJob) {
  const api = yield* Api
  return yield* api
    .request('models.setImages', { provider: job.provider, model: job.modelId, images: job.images })
    .pipe(Effect.tapError(error => notify(error.message)))
})

const setContextLimit = Effect.fn('ModelsSettings.setContextLimit')(function* (job: ContextJob) {
  const api = yield* Api
  return yield* api.request('models.setContextLimit', {
    provider: job.provider,
    model: job.modelId,
    limit: job.limit,
  })
})

export const ModelsInput = Models
export type ModelsInput = typeof ModelsInput.Type

export const ModelsStates = Machine.state({
  type: 'parallel',
  fields: rootFields,
  states: {
    Sync: { states: { Idle: {}, Refreshing: {} } },
    Main: {
      states: {
        Idle: {},
        Saving: { fields: { submitted: Slot, compactionVersion: Schema.Finite } },
      },
    },
    Compaction: {
      states: {
        Idle: {},
        Saving: { fields: { submitted: Schema.NullOr(Slot) } },
      },
    },
    Utility: {
      states: {
        Idle: {},
        Saving: { fields: { submitted: Schema.NullOr(Slot) } },
      },
    },
    Images: { states: { Idle: {}, Saving: {} } },
    Context: { states: { Idle: {}, Saving: {} } },
  },
})

export const ModelsEvents = Machine.events({
  Refresh: {},
  AssignMain: { slot: Slot },
  AssignCompaction: { slot: Schema.NullOr(Slot) },
  AssignUtility: { slot: Schema.NullOr(Slot) },
  ToggleImages: { provider: Schema.String, modelId: Schema.String, images: Schema.Boolean },
  UpdateContext: { provider: Schema.String, modelId: Schema.String, value: Schema.String },
  FlushContext: { provider: Schema.String, modelId: Schema.String },
})

export const ModelsMachine = Machine.make({
  id: 'ModelsSettings',
  root: ModelsStates,
  events: ModelsEvents,
  input: ModelsInput,
  effects: { fetchModels, setMain, setCompaction, setUtility, setImages, setContextLimit },
}).handle({
  root: ({ input }) => ({
    server: input,
    shown: input,
    roleStatuses: {},
    compactionVersion: 0,
    contextDrafts: {},
    imageJobs: [],
    contextJobs: [],
  }),
  on: {
    ToggleImages: {
      update: 'root',
      data: ({ event, root }) => ({
        ...root,
        shown: withImages(root.shown, event.provider, event.modelId, event.images),
        imageJobs: [
          ...root.imageJobs,
          { provider: event.provider, modelId: event.modelId, images: event.images },
        ],
      }),
    },
    UpdateContext: {
      update: 'root',
      data: ({ event, root }) => {
        const key = contextKey(event.provider, event.modelId)
        const previous = root.contextDrafts[key]
        return {
          ...root,
          contextDrafts: {
            ...root.contextDrafts,
            [key]: {
              value: event.value,
              version: (previous?.version ?? 0) + 1,
              status: '',
            },
          },
        }
      },
    },
    FlushContext: {
      update: 'root',
      data: ({ event, root }) => flushContextDraft(event, root),
    },
  },
  states: {
    Sync: {
      initial: { target: 'Sync.Idle' },
      states: {
        Idle: {
          on: {
            Refresh: { target: 'Sync.Refreshing' },
          },
        },
        Refreshing: {
          invoke: {
            src: 'fetchModels',
            onDone: {
              target: 'Sync.Idle',
              update: 'root',
              data: ({ root, output }) => {
                if (output === null) return { target: undefined, update: root }
                const shown: Models = {
                  providers: root.imageJobs.reduce(
                    (models, job) => withImages(models, job.provider, job.modelId, job.images),
                    output,
                  ).providers,
                  main:
                    root.roleStatuses['main'] === 'Saving…' ||
                    !sameSlot(root.shown.main, root.server.main)
                      ? root.shown.main
                      : output.main,
                  compaction:
                    root.roleStatuses['compaction'] === 'Saving…' ||
                    !sameSlot(root.shown.compaction, root.server.compaction)
                      ? root.shown.compaction
                      : output.compaction,
                  utility:
                    root.roleStatuses['utility'] === 'Saving…' ||
                    !sameSlot(root.shown.utility, root.server.utility)
                      ? root.shown.utility
                      : output.utility,
                }
                return { target: undefined, update: { ...root, server: output, shown } }
              },
            },
          },
        },
      },
    },
    Main: {
      initial: { target: 'Main.Idle' },
      states: {
        Idle: {
          always: {
            target: 'Main.Saving',
            guard: ({ root }) => !sameSlot(root.shown.main, root.server.main),
            data: ({ root }) => ({
              submitted: root.shown.main,
              compactionVersion: root.compactionVersion,
            }),
          },
          on: {
            AssignMain: {
              target: 'Main.Saving',
              update: 'root',
              data: ({ event, root }) => ({
                target: { submitted: event.slot, compactionVersion: root.compactionVersion },
                update: {
                  ...root,
                  shown: { ...root.shown, main: event.slot },
                  roleStatuses: { ...root.roleStatuses, main: 'Saving…' },
                },
              }),
            },
          },
        },
        Saving: {
          on: {
            AssignMain: {
              update: 'root',
              data: ({ event, root }) => ({
                ...root,
                shown: { ...root.shown, main: event.slot },
                roleStatuses: { ...root.roleStatuses, main: 'Saving…' },
              }),
            },
          },
          invoke: {
            src: 'setMain',
            input: ({ state }) => state.submitted,
            onDone: {
              target: 'Main.Idle',
              update: 'root',
              data: ({ root, state, output }) => ({
                target: undefined,
                update: {
                  ...root,
                  ...confirmMain(root, output, state.submitted, state.compactionVersion),
                  roleStatuses: { ...root.roleStatuses, main: 'Saved' },
                },
              }),
            },
            onFailure: {
              target: 'Main.Idle',
              update: 'root',
              data: ({ root, state, error }) => ({
                target: undefined,
                update: {
                  ...root,
                  shown: {
                    ...root.shown,
                    main: sameSlot(root.shown.main, state.submitted)
                      ? root.server.main
                      : root.shown.main,
                  },
                  roleStatuses: { ...root.roleStatuses, main: error.message },
                },
              }),
            },
          },
        },
      },
    },
    Compaction: {
      initial: { target: 'Compaction.Idle' },
      states: {
        Idle: {
          always: {
            target: 'Compaction.Saving',
            guard: ({ root }) => !sameSlot(root.shown.compaction, root.server.compaction),
            data: ({ root }) => ({ submitted: root.shown.compaction }),
          },
          on: {
            AssignCompaction: {
              target: 'Compaction.Saving',
              update: 'root',
              data: ({ event, root }) => ({
                target: { submitted: event.slot },
                update: {
                  ...root,
                  shown: { ...root.shown, compaction: event.slot },
                  roleStatuses: { ...root.roleStatuses, compaction: 'Saving…' },
                },
              }),
            },
          },
        },
        Saving: {
          on: {
            AssignCompaction: {
              update: 'root',
              data: ({ event, root }) => ({
                ...root,
                shown: { ...root.shown, compaction: event.slot },
                roleStatuses: { ...root.roleStatuses, compaction: 'Saving…' },
              }),
            },
          },
          invoke: {
            src: 'setCompaction',
            input: ({ state }) => state.submitted,
            onDone: {
              target: 'Compaction.Idle',
              update: 'root',
              data: ({ root, state, output }) => ({
                target: undefined,
                update: {
                  ...root,
                  ...confirmRole(root, 'compaction', output, state.submitted),
                  compactionVersion: root.compactionVersion + 1,
                  roleStatuses: { ...root.roleStatuses, compaction: 'Saved' },
                },
              }),
            },
            onFailure: {
              target: 'Compaction.Idle',
              update: 'root',
              data: ({ root, state, error }) => ({
                target: undefined,
                update: {
                  ...root,
                  shown: {
                    ...root.shown,
                    compaction: sameSlot(root.shown.compaction, state.submitted)
                      ? root.server.compaction
                      : root.shown.compaction,
                  },
                  roleStatuses: { ...root.roleStatuses, compaction: error.message },
                },
              }),
            },
          },
        },
      },
    },
    Utility: {
      initial: { target: 'Utility.Idle' },
      states: {
        Idle: {
          always: {
            target: 'Utility.Saving',
            guard: ({ root }) => !sameSlot(root.shown.utility, root.server.utility),
            data: ({ root }) => ({ submitted: root.shown.utility }),
          },
          on: {
            AssignUtility: {
              target: 'Utility.Saving',
              update: 'root',
              data: ({ event, root }) => ({
                target: { submitted: event.slot },
                update: {
                  ...root,
                  shown: { ...root.shown, utility: event.slot },
                  roleStatuses: { ...root.roleStatuses, utility: 'Saving…' },
                },
              }),
            },
          },
        },
        Saving: {
          on: {
            AssignUtility: {
              update: 'root',
              data: ({ event, root }) => ({
                ...root,
                shown: { ...root.shown, utility: event.slot },
                roleStatuses: { ...root.roleStatuses, utility: 'Saving…' },
              }),
            },
          },
          invoke: {
            src: 'setUtility',
            input: ({ state }) => state.submitted,
            onDone: {
              target: 'Utility.Idle',
              update: 'root',
              data: ({ root, state, output }) => ({
                target: undefined,
                update: {
                  ...root,
                  ...confirmRole(root, 'utility', output, state.submitted),
                  roleStatuses: { ...root.roleStatuses, utility: 'Saved' },
                },
              }),
            },
            onFailure: {
              target: 'Utility.Idle',
              update: 'root',
              data: ({ root, state, error }) => ({
                target: undefined,
                update: {
                  ...root,
                  shown: {
                    ...root.shown,
                    utility: sameSlot(root.shown.utility, state.submitted)
                      ? root.server.utility
                      : root.shown.utility,
                  },
                  roleStatuses: { ...root.roleStatuses, utility: error.message },
                },
              }),
            },
          },
        },
      },
    },
    Images: {
      initial: { target: 'Images.Idle' },
      states: {
        Idle: {
          always: {
            target: 'Images.Saving',
            guard: ({ root }) => root.imageJobs.length > 0,
          },
        },
        Saving: {
          invoke: {
            src: 'setImages',
            input: ({ root }) => Option.getOrThrow(Array.head(root.imageJobs)),
            onDone: {
              target: 'Images.Idle',
              update: 'root',
              data: ({ root, output }) => ({
                target: undefined,
                update: { ...root, ...settleImages(root, output) },
              }),
            },
            onFailure: {
              target: 'Images.Idle',
              update: 'root',
              data: ({ root }) => ({
                target: undefined,
                update: { ...root, ...settleImages(root, null) },
              }),
            },
          },
        },
      },
    },
    Context: {
      initial: { target: 'Context.Idle' },
      states: {
        Idle: {
          always: {
            target: 'Context.Saving',
            guard: ({ root }) => root.contextJobs.length > 0,
          },
        },
        Saving: {
          invoke: {
            src: 'setContextLimit',
            input: ({ root }) => Option.getOrThrow(Array.head(root.contextJobs)),
            onDone: {
              target: 'Context.Idle',
              update: 'root',
              data: ({ root, output }) => ({
                target: undefined,
                update: settleContext(root, output, 'Saved'),
              }),
            },
            onFailure: {
              target: 'Context.Idle',
              update: 'root',
              data: ({ root, error }) => ({
                target: undefined,
                update: settleContext(root, null, error.message),
              }),
            },
          },
        },
      },
    },
  },
})
