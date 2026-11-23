import { Api } from '@blob-1/client/api'
import { GeneralSettings } from '@blob-1/contracts'
import { Machine } from '@typeonce/effect-machine'
import { Effect, Schema } from 'effect'

import { Notify } from '~/lib/notify'

const fetchGeneral = Effect.gen(function* () {
  const api = yield* Api
  return yield* api.request('general.get', {}).pipe(Effect.orElseSucceed(() => null))
})

const saveAutoRespond = Effect.fn('GeneralSettings.setAutoRespond')(function* (enabled: boolean) {
  const api = yield* Api
  const notify = yield* Notify
  return yield* api
    .request('general.setAutoRespond', { enabled })
    .pipe(Effect.catch(error => notify.error(error.message).pipe(Effect.as(null))))
})

export const GeneralInput = GeneralSettings
export type GeneralInput = typeof GeneralInput.Type

export const GeneralStates = Machine.state({
  type: 'parallel',
  fields: {
    serverAutoRespond: Schema.Boolean,
    autoRespond: Schema.Boolean,
    hasUtilityModel: Schema.Boolean,
    inFlightSave: Schema.Boolean,
    mutationSeq: Schema.Finite,
  },
  states: {
    Save: {
      states: {
        Idle: {},
        Saving: { fields: { enabled: Schema.Boolean } },
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

export const GeneralEvents = Machine.events({
  SetAutoRespond: { enabled: Schema.Boolean },
  Refresh: {},
})

/** General bot-wide settings with optimistic Auto-respond toggling and coalesced saves. */
export const GeneralMachine = Machine.make({
  id: 'GeneralSettings',
  root: GeneralStates,
  events: GeneralEvents,
  input: GeneralInput,
  effects: { saveAutoRespond, fetchGeneral },
}).handle({
  root: ({ input }) => ({
    serverAutoRespond: input.autoRespond,
    autoRespond: input.autoRespond,
    hasUtilityModel: input.hasUtilityModel,
    inFlightSave: false,
    mutationSeq: 0,
  }),
  states: {
    Save: {
      initial: { target: 'Save.Idle' },
      states: {
        Idle: {
          always: {
            guard: ({ root }) => root.autoRespond !== root.serverAutoRespond,
            target: 'Save.Saving',
            update: 'root',
            data: ({ root }) => ({
              target: { enabled: root.autoRespond },
              update: { ...root, inFlightSave: true, mutationSeq: root.mutationSeq + 1 },
            }),
          },
          on: {
            SetAutoRespond: {
              target: 'Save.Saving',
              update: 'root',
              data: ({ root, event }) => ({
                target: { enabled: event.enabled },
                update: {
                  ...root,
                  autoRespond: event.enabled,
                  inFlightSave: true,
                  mutationSeq: root.mutationSeq + 1,
                },
              }),
            },
          },
        },
        Saving: {
          on: {
            SetAutoRespond: {
              update: 'root',
              data: ({ root, event }) => ({
                ...root,
                autoRespond: event.enabled,
                mutationSeq: root.mutationSeq + 1,
              }),
            },
          },
          invoke: {
            src: 'saveAutoRespond',
            input: ({ state }) => state.enabled,
            onDone: {
              target: 'Save.Idle',
              update: 'root',
              data: ({ root, state, output }) => {
                if (output === null) {
                  return {
                    target: undefined,
                    update: {
                      ...root,
                      inFlightSave: false,
                      autoRespond:
                        root.autoRespond === state.enabled
                          ? root.serverAutoRespond
                          : root.autoRespond,
                    },
                  }
                }
                return {
                  target: undefined,
                  update: {
                    ...root,
                    inFlightSave: false,
                    serverAutoRespond: output.autoRespond,
                    autoRespond:
                      root.autoRespond === state.enabled ? output.autoRespond : root.autoRespond,
                    hasUtilityModel: output.hasUtilityModel,
                    mutationSeq: root.mutationSeq + 1,
                  },
                }
              },
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
            src: 'fetchGeneral',
            onDone: {
              target: 'Sync.Idle',
              update: 'root',
              data: ({ root, state, output }) => ({
                target: undefined,
                update:
                  output === null
                    ? root
                    : root.inFlightSave ||
                        state.mutationSeq !== root.mutationSeq ||
                        root.autoRespond !== root.serverAutoRespond
                      ? { ...root, hasUtilityModel: output.hasUtilityModel }
                      : {
                          ...root,
                          serverAutoRespond: output.autoRespond,
                          autoRespond: output.autoRespond,
                          hasUtilityModel: output.hasUtilityModel,
                        },
              }),
            },
          },
        },
      },
    },
  },
})
