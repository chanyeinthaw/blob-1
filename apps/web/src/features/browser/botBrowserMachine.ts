import { Api } from '@blob-1/client/api'
import { BotBrowser } from '@blob-1/contracts'
import { Machine } from '@typeonce/effect-machine'
import { Effect, Schema } from 'effect'

import { Notify } from '~/lib/notify'

const fetchBotBrowser = Effect.gen(function* () {
  const api = yield* Api
  return yield* api.request('bot.browser.get', {}).pipe(Effect.orElseSucceed(() => null))
})

const saveBrowserProfile = Effect.fn('BotBrowser.setProfile')(function* (profile: string | null) {
  const api = yield* Api
  const notify = yield* Notify
  return yield* api
    .request('bot.browser.set', { profile })
    .pipe(Effect.catch(error => notify.error(error.message).pipe(Effect.as(null))))
})

export const BotBrowserInput = BotBrowser
export type BotBrowserInput = typeof BotBrowserInput.Type

export const BotBrowserStates = Machine.state({
  type: 'parallel',
  fields: {
    serverData: BotBrowser,
    data: BotBrowser,
    inFlightSave: Schema.Boolean,
    mutationSeq: Schema.Finite,
  },
  states: {
    Mutate: {
      states: {
        Idle: {},
        Saving: { fields: { profile: Schema.NullOr(Schema.String) } },
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

export const BotBrowserEvents = Machine.events({
  SetProfile: { profile: Schema.NullOr(Schema.String) },
  Refresh: {},
})

/** Per-bot browser profile selection with optimistic saves and refresh race protection. */
export const BotBrowserMachine = Machine.make({
  id: 'BotBrowser',
  root: BotBrowserStates,
  events: BotBrowserEvents,
  input: BotBrowserInput,
  effects: { fetchBotBrowser, saveBrowserProfile },
}).handle({
  root: ({ input }) => ({ serverData: input, data: input, inFlightSave: false, mutationSeq: 0 }),
  states: {
    Mutate: {
      initial: { target: 'Mutate.Idle' },
      states: {
        Idle: {
          on: {
            SetProfile: {
              target: 'Mutate.Saving',
              update: 'root',
              data: ({ root, event }) => ({
                target: { profile: event.profile },
                update: {
                  ...root,
                  data: { ...root.data, profile: event.profile },
                  inFlightSave: true,
                  mutationSeq: root.mutationSeq + 1,
                },
              }),
            },
          },
        },
        Saving: {
          invoke: {
            src: 'saveBrowserProfile',
            input: ({ state }) => state.profile,
            onDone: {
              target: 'Mutate.Idle',
              update: 'root',
              data: ({ root, output }) => ({
                target: undefined,
                update:
                  output === null
                    ? { ...root, data: root.serverData, inFlightSave: false }
                    : {
                        ...root,
                        serverData: output,
                        data: output,
                        inFlightSave: false,
                        mutationSeq: root.mutationSeq + 1,
                      },
              }),
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
            src: 'fetchBotBrowser',
            onDone: {
              target: 'Sync.Idle',
              update: 'root',
              data: ({ root, state, output }) => ({
                target: undefined,
                update:
                  output !== null && state.mutationSeq === root.mutationSeq
                    ? {
                        ...root,
                        serverData: output,
                        data: root.inFlightSave ? root.data : output,
                      }
                    : root,
              }),
            },
          },
        },
      },
    },
  },
})
