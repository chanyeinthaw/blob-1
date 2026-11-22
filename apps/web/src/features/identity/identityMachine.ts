import { Api } from '@blob-1/client/api'
import { Identity } from '@blob-1/contracts'
import { Machine } from '@typeonce/effect-machine'
import { Effect, Schema } from 'effect'

const saveIdentity = Effect.fn('IdentitySettings.save')(function* (text: string) {
  const api = yield* Api
  return yield* api.request('identity.set', { text })
})

const resetIdentity = Effect.flatMap(Api, api => api.request('identity.reset', {}))

export const IdentityInput = Identity
export type IdentityInput = typeof IdentityInput.Type

const FailedAction = Schema.Literals(['save', 'reset'])

const rootFields = {
  savedText: Schema.String,
  draftText: Schema.String,
  isDefault: Schema.Boolean,
  hasSaved: Schema.Boolean,
  pendingFlush: Schema.Boolean,
  error: Schema.NullOr(Schema.String),
  failedAction: Schema.NullOr(FailedAction),
}
const RootData = Schema.Struct(rootFields)
type RootData = typeof RootData.Type

const applyEdit = (root: RootData, text: string): RootData => ({
  ...root,
  draftText: text,
  hasSaved: false,
  error: text.trim() === '' ? "Identity can't be blank." : null,
  failedAction: null,
})

export const IdentityStates = Machine.state({
  fields: rootFields,
  states: {
    Idle: {},
    Debouncing: {},
    Saving: { fields: { text: Schema.String } },
    Resetting: {},
  },
})

export const IdentityEvents = Machine.events({
  Edit: { text: Schema.String },
  Flush: {},
  Reset: {},
})

/** The identity editor: autosaves 800ms after typing stops, on blur, and resets on demand. */
export const IdentityMachine = Machine.make({
  id: 'IdentitySettings',
  root: IdentityStates,
  events: IdentityEvents,
  input: IdentityInput,
  effects: { saveIdentity, resetIdentity },
  timers: { autosaveDelay: '800 millis' },
  branches: {
    flush: {
      save: { target: 'Saving', update: 'root' },
      settle: { target: 'Idle' },
    },
    saved: {
      clean: { target: 'Idle', update: 'root' },
      dirty: { target: 'Debouncing', update: 'root' },
    },
    failed: {
      failed: { target: 'Idle', update: 'root' },
      retry: { target: 'Debouncing', update: 'root' },
    },
  },
}).handle({
  root: ({ input }) => ({
    savedText: input.text,
    draftText: input.text,
    isDefault: input.isDefault,
    hasSaved: false,
    pendingFlush: false,
    error: null,
    failedAction: null,
  }),
  initial: { target: 'Idle' },
  states: {
    Idle: {
      on: {
        Edit: {
          target: 'Debouncing',
          update: 'root',
          data: ({ event, root }) => ({
            target: undefined,
            update: applyEdit(root, event.text),
          }),
        },
        Flush: {
          branches: 'flush',
          resolve: ({ root, select }) =>
            root.draftText.trim() === '' || root.draftText === root.savedText
              ? select.settle()
              : select.save({
                  data: { text: root.draftText },
                  update: { data: { ...root, error: null, failedAction: null } },
                }),
        },
        Reset: {
          target: 'Resetting',
          update: 'root',
          data: ({ root }) => ({
            target: undefined,
            update: { ...root, error: null, failedAction: null },
          }),
        },
      },
    },
    Debouncing: {
      invoke: {
        src: 'autosaveDelay',
        onDone: {
          branches: 'flush',
          resolve: ({ root, select }) =>
            root.draftText.trim() === '' || root.draftText === root.savedText
              ? select.settle()
              : select.save({
                  data: { text: root.draftText },
                  update: { data: { ...root, error: null, failedAction: null } },
                }),
        },
      },
      on: {
        Edit: {
          target: 'Debouncing',
          reenter: true,
          update: 'root',
          data: ({ event, root }) => ({
            target: undefined,
            update: applyEdit(root, event.text),
          }),
        },
        Flush: {
          branches: 'flush',
          resolve: ({ root, select }) =>
            root.draftText.trim() === '' || root.draftText === root.savedText
              ? select.settle()
              : select.save({
                  data: { text: root.draftText },
                  update: { data: { ...root, error: null, failedAction: null } },
                }),
        },
        Reset: {
          target: 'Resetting',
          update: 'root',
          data: ({ root }) => ({
            target: undefined,
            update: { ...root, error: null, failedAction: null },
          }),
        },
      },
    },
    Saving: {
      on: {
        Edit: {
          update: 'root',
          data: ({ event, root }) => applyEdit(root, event.text),
        },
        Flush: {
          update: 'root',
          data: ({ root }) => ({ ...root, pendingFlush: true }),
        },
      },
      invoke: {
        src: 'saveIdentity',
        input: ({ state }) => state.text,
        onDone: {
          branches: 'saved',
          resolve: ({ root, output, state, select }, enqueue) => {
            const confirmed = {
              ...root,
              savedText: output.text,
              isDefault: output.isDefault,
              pendingFlush: false,
            }
            if (root.draftText === state.text) {
              return select.clean({
                update: {
                  data: {
                    ...confirmed,
                    draftText: output.text,
                    hasSaved: true,
                    error: null,
                    failedAction: null,
                  },
                },
              })
            }
            // A flush queued during the write re-fires so the latest draft saves now.
            if (root.pendingFlush) {
              enqueue.raise(IdentityEvents.Flush())
            }
            return select.dirty({
              update: { data: { ...confirmed, hasSaved: root.draftText === output.text } },
            })
          },
        },
        onFailure: {
          branches: 'failed',
          resolve: ({ root, state, error, select }, enqueue) => {
            const changed = root.draftText !== state.text
            if (!changed) {
              return select.failed({
                update: {
                  data: {
                    ...root,
                    pendingFlush: false,
                    error: error.message,
                    failedAction: 'save',
                  },
                },
              })
            }
            // A newer draft still saves: now if it was already flushed, else after the debounce.
            // It keeps its own validation error.
            if (root.pendingFlush) {
              enqueue.raise(IdentityEvents.Flush())
              return select.failed({ update: { data: { ...root, pendingFlush: false } } })
            }
            return select.retry({ update: { data: { ...root, pendingFlush: false } } })
          },
        },
      },
    },
    Resetting: {
      on: {
        Edit: {
          update: 'root',
          data: ({ event, root }) => applyEdit(root, event.text),
        },
      },
      invoke: {
        src: 'resetIdentity',
        onDone: {
          target: 'Idle',
          update: 'root',
          data: ({ output }) => ({
            target: undefined,
            update: {
              savedText: output.text,
              draftText: output.text,
              isDefault: output.isDefault,
              hasSaved: true,
              pendingFlush: false,
              error: null,
              failedAction: null,
            },
          }),
        },
        onFailure: {
          target: 'Idle',
          update: 'root',
          data: ({ root, error }) => ({
            target: undefined,
            update: { ...root, error: error.message, failedAction: 'reset' },
          }),
        },
      },
    },
  },
})
