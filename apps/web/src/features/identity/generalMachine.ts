import { Api } from '@blob-1/client/api'
import { GeneralSettings } from '@blob-1/contracts'
import { Machine } from '@typeonce/effect-machine'
import { Effect, Schema } from 'effect'

import { Notify } from '~/lib/notify'

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
  fields: {
    autoRespond: Schema.Boolean,
    hasUtilityModel: Schema.Boolean,
  },
  states: {
    Idle: {},
    Saving: { fields: { enabled: Schema.Boolean } },
  },
})

export const GeneralEvents = Machine.events({
  SetAutoRespond: { enabled: Schema.Boolean },
})

/** General bot-wide settings (Auto-respond utility-model gate). */
export const GeneralMachine = Machine.make({
  id: 'GeneralSettings',
  root: GeneralStates,
  events: GeneralEvents,
  input: GeneralInput,
  effects: { saveAutoRespond },
}).handle({
  root: ({ input }) => ({
    autoRespond: input.autoRespond,
    hasUtilityModel: input.hasUtilityModel,
  }),
  initial: { target: 'Idle' },
  states: {
    Idle: {
      on: {
        SetAutoRespond: {
          target: 'Saving',
          data: ({ event }) => ({ enabled: event.enabled }),
        },
      },
    },
    Saving: {
      invoke: {
        src: 'saveAutoRespond',
        input: ({ state }) => state.enabled,
        onDone: {
          target: 'Idle',
          update: 'root',
          data: ({ root, output }) => ({
            target: undefined,
            update: output ?? root,
          }),
        },
      },
    },
  },
})
