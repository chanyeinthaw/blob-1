import { Api } from '@blob-1/client/api'
import {
  Discord,
  DiscordDirectory,
  DiscordInteractionMode,
  DiscordInteractionScope,
  Home,
} from '@blob-1/contracts'
import { Machine } from '@typeonce/effect-machine'
import { Effect, Schedule, Schema } from 'effect'

const emptyDirectory: DiscordDirectory = { channels: [], people: [] }

const saveToken = Effect.fn('DiscordSettings.saveToken')(function* (token: string) {
  const api = yield* Api
  return yield* api.request('discord.setToken', { token })
})

const removeToken = Effect.flatMap(Api, api => api.request('discord.clearToken', {}))

const allow = Effect.fn('DiscordSettings.allow')(function* (user: string) {
  const api = yield* Api
  return yield* api.request('discord.allow', { user })
})

const disallow = Effect.fn('DiscordSettings.disallow')(function* (user: string) {
  const api = yield* Api
  return yield* api.request('discord.disallow', { user })
})

const setHome = Effect.fn('DiscordSettings.setHome')(function* (home: Home) {
  const api = yield* Api
  return yield* api.request('discord.setHome', { home })
})

const setInteractionMode = Effect.fn('DiscordSettings.setInteractionMode')(function* (input: {
  readonly scope: DiscordInteractionScope
  readonly targetId: string
  readonly mode: DiscordInteractionMode
}) {
  const api = yield* Api
  return yield* api.request('discord.setInteractionMode', input)
})

const clearInteractionMode = Effect.fn('DiscordSettings.clearInteractionMode')(function* (input: {
  readonly scope: DiscordInteractionScope
  readonly targetId: string
}) {
  const api = yield* Api
  return yield* api.request('discord.clearInteractionMode', input)
})

/** Polls every 2 seconds until the bot leaves "connecting", then loads the new bot's directory. */
const watchConnection = Effect.gen(function* () {
  const api = yield* Api
  const discord = yield* Effect.sleep('2 seconds').pipe(
    Effect.andThen(api.request('discord.get', {})),
    Effect.retry(Schedule.spaced('2 seconds')),
    Effect.repeat({ until: next => next.connection.state !== 'connecting' }),
  )
  const directory =
    discord.connection.state === 'online'
      ? yield* api.request('discord.directory', {}).pipe(Effect.orElseSucceed(() => emptyDirectory))
      : emptyDirectory
  return { discord, directory }
})

/** A token reply owns only the token and connection; other fields may have changed since. */
const withToken = (current: Discord, reply: Discord): Discord => ({
  ...current,
  token: reply.token,
  connection: reply.connection,
})

export const DiscordInput = Schema.Struct({ discord: Discord, directory: DiscordDirectory })
export type DiscordInput = typeof DiscordInput.Type

const ErrorMessage = Schema.NullOr(Schema.String)
const HomeKind = Schema.Literals(['none', 'channel', 'dm'])

export const DiscordStates = Machine.state({
  type: 'parallel',
  fields: {
    discord: Discord,
    directory: DiscordDirectory,
    /** Bumped on every token save or removal; a connection watch started earlier is stale. */
    tokenGeneration: Schema.Finite,
    /** Transient home type chosen before picking a destination; null follows `discord.home.kind`. */
    pendingHomeKind: Schema.NullOr(HomeKind),
    /** Add-override form draft; root fields so busy states can't lose it. */
    modeScope: DiscordInteractionScope,
    modeTarget: Schema.String,
    modeSetting: DiscordInteractionMode,
  },
  states: {
    Token: {
      states: {
        Saved: {},
        Editing: { fields: { error: ErrorMessage } },
        Saving: { fields: { token: Schema.String } },
        Removing: {},
      },
    },
    Status: {
      states: {
        Settled: {},
        Watching: { fields: { tokenGeneration: Schema.Finite } },
      },
    },
    Allowlist: {
      states: {
        Idle: { fields: { draft: Schema.String, error: ErrorMessage } },
        Adding: { fields: { user: Schema.String } },
        Removing: { fields: { user: Schema.String, draft: Schema.String } },
      },
    },
    HomeChannel: {
      states: {
        Idle: {
          fields: {
            error: ErrorMessage,
            saved: Schema.Boolean,
            queued: Schema.NullOr(Home),
            failedHome: Schema.NullOr(Home),
          },
        },
        Saving: { fields: { home: Home, queued: Schema.NullOr(Home) } },
      },
    },
    Interaction: {
      states: {
        Idle: { fields: { error: ErrorMessage } },
        Saving: {
          fields: {
            scope: DiscordInteractionScope,
            targetId: Schema.String,
            mode: DiscordInteractionMode,
          },
        },
        Removing: {
          fields: { scope: DiscordInteractionScope, targetId: Schema.String },
        },
      },
    },
  },
})

export const DiscordEvents = Machine.events({
  ReplaceToken: {},
  CancelToken: {},
  ClearTokenError: {},
  SubmitToken: { token: Schema.String },
  RemoveToken: {},
  EditAllowDraft: { draft: Schema.String },
  Allow: { user: Schema.String },
  Disallow: { user: Schema.String },
  SelectHomeKind: { kind: HomeKind },
  SetHome: { home: Home },
  SelectModeScope: { scope: DiscordInteractionScope },
  SelectModeTarget: { targetId: Schema.String },
  SelectModeSetting: { mode: DiscordInteractionMode },
  SetInteractionMode: {
    scope: DiscordInteractionScope,
    targetId: Schema.String,
    mode: DiscordInteractionMode,
  },
  ClearInteractionMode: { scope: DiscordInteractionScope, targetId: Schema.String },
})

/** The Discord settings page: token, live connection status, allowlist and home channel. */
export const DiscordMachine = Machine.make({
  id: 'DiscordSettings',
  root: DiscordStates,
  events: DiscordEvents,
  input: DiscordInput,
  effects: {
    saveToken,
    removeToken,
    watchConnection,
    allow,
    disallow,
    setHome,
    setInteractionMode,
    clearInteractionMode,
  },
}).handle({
  root: ({ input }) => ({
    ...input,
    tokenGeneration: 0,
    pendingHomeKind: null,
    modeScope: 'channel',
    modeTarget: '',
    modeSetting: 'mention-only',
  }),
  states: {
    Token: {
      initial: { target: 'Token.Saved' },
      states: {
        Saved: {
          always: {
            target: 'Token.Editing',
            guard: ({ root }) => root.discord.token === 'missing',
            data: { error: null },
          },
          on: {
            ReplaceToken: { target: 'Token.Editing', data: { error: null } },
            RemoveToken: { target: 'Token.Removing' },
          },
        },
        Editing: {
          on: {
            ClearTokenError: {
              update: 'Token.Editing',
              data: { error: null },
            },
            CancelToken: {
              target: 'Token.Saved',
              guard: ({ root }) => root.discord.token === 'set',
            },
            SubmitToken: {
              target: 'Token.Saving',
              guard: ({ event }) => event.token.trim() !== '',
              data: ({ event }) => ({ token: event.token.trim() }),
            },
          },
        },
        Saving: {
          invoke: {
            src: 'saveToken',
            input: ({ state }) => state.token,
            onDone: {
              target: 'Token.Saved',
              update: 'root',
              data: ({ root, output }) => ({
                target: undefined,
                update:
                  output.connection.state === 'online'
                    ? { ...root, discord: withToken(root.discord, output) }
                    : {
                        ...root,
                        discord: withToken(root.discord, output),
                        directory: emptyDirectory,
                        tokenGeneration: root.tokenGeneration + 1,
                      },
              }),
            },
            onFailure: {
              target: 'Token.Editing',
              data: ({ error }) => ({ error: error.message }),
            },
          },
        },
        Removing: {
          invoke: {
            src: 'removeToken',
            onDone: {
              target: 'Token.Editing',
              update: 'root',
              data: ({ root, output }) => ({
                target: { error: null },
                update: {
                  ...root,
                  discord: withToken(root.discord, output),
                  directory: emptyDirectory,
                  tokenGeneration: root.tokenGeneration + 1,
                },
              }),
            },
            onFailure: {
              target: 'Token.Editing',
              data: ({ error }) => ({ error: error.message }),
            },
          },
        },
      },
    },
    Status: {
      initial: { target: 'Status.Settled' },
      states: {
        Settled: {
          always: {
            target: 'Status.Watching',
            guard: ({ root }) => root.discord.connection.state === 'connecting',
            data: ({ root }) => ({ tokenGeneration: root.tokenGeneration }),
          },
        },
        Watching: {
          invoke: {
            src: 'watchConnection',
            onDone: {
              target: 'Status.Settled',
              update: 'root',
              data: ({ root, state, output }) => ({
                target: undefined,
                update:
                  state.tokenGeneration === root.tokenGeneration
                    ? {
                        ...root,
                        discord: { ...root.discord, connection: output.discord.connection },
                        directory: output.directory,
                      }
                    : root,
              }),
            },
            onFailure: { target: 'Status.Settled' },
          },
        },
      },
    },
    Allowlist: {
      initial: { target: 'Allowlist.Idle', data: { draft: '', error: null } },
      states: {
        Idle: {
          on: {
            EditAllowDraft: {
              update: 'Allowlist.Idle',
              data: ({ event }) => ({ draft: event.draft, error: null }),
            },
            Allow: {
              target: 'Allowlist.Adding',
              data: ({ event }) => ({ user: event.user }),
            },
            Disallow: {
              target: 'Allowlist.Removing',
              data: ({ state, event }) => ({ user: event.user, draft: state.draft }),
            },
          },
        },
        Adding: {
          invoke: {
            src: 'allow',
            input: ({ state }) => state.user,
            onDone: {
              target: 'Allowlist.Idle',
              update: 'root',
              data: ({ root, output }) => ({
                target: { draft: '', error: null },
                update: { ...root, discord: { ...root.discord, allowed: output.allowed } },
              }),
            },
            onFailure: {
              target: 'Allowlist.Idle',
              data: ({ state, error }) => ({ draft: state.user, error: error.message }),
            },
          },
        },
        Removing: {
          invoke: {
            src: 'disallow',
            input: ({ state }) => state.user,
            onDone: {
              target: 'Allowlist.Idle',
              update: 'root',
              data: ({ root, state, output }) => ({
                target: { draft: state.draft, error: null },
                update: { ...root, discord: { ...root.discord, allowed: output.allowed } },
              }),
            },
            onFailure: {
              target: 'Allowlist.Idle',
              data: ({ state, error }) => ({ draft: state.draft, error: error.message }),
            },
          },
        },
      },
    },
    HomeChannel: {
      initial: {
        target: 'HomeChannel.Idle',
        data: { error: null, saved: false, queued: null, failedHome: null },
      },
      states: {
        Idle: {
          always: {
            target: 'HomeChannel.Saving',
            guard: ({ state }) => state.queued !== null,
            data: ({ state }) => ({ home: state.queued ?? { kind: 'none' }, queued: null }),
          },
          on: {
            SelectHomeKind: {
              target: 'HomeChannel.Idle',
              update: 'root',
              data: ({ event, root, state }) => ({
                target: { ...state, error: null, failedHome: null },
                update: {
                  ...root,
                  pendingHomeKind: event.kind === root.discord.home.kind ? null : event.kind,
                },
              }),
            },
            SetHome: {
              target: 'HomeChannel.Saving',
              data: ({ event }) => ({ home: event.home, queued: null }),
            },
          },
        },
        Saving: {
          on: {
            SetHome: {
              update: 'HomeChannel.Saving',
              data: ({ state, event }) => ({ home: state.home, queued: event.home }),
            },
          },
          invoke: {
            src: 'setHome',
            input: ({ state }) => state.home,
            onDone: {
              target: 'HomeChannel.Idle',
              update: 'root',
              data: ({ root, state, output }) => ({
                target: { error: null, saved: true, queued: state.queued, failedHome: null },
                update: {
                  ...root,
                  discord: { ...root.discord, home: output.home },
                  pendingHomeKind: state.queued === null ? null : root.pendingHomeKind,
                },
              }),
            },
            onFailure: {
              target: 'HomeChannel.Idle',
              update: 'root',
              data: ({ root, state, error }) => ({
                target: {
                  error: error.message,
                  saved: false,
                  queued: state.queued,
                  failedHome: state.home,
                },
                update: {
                  ...root,
                  pendingHomeKind: state.home.kind === 'none' ? null : root.pendingHomeKind,
                },
              }),
            },
          },
        },
      },
    },
    Interaction: {
      initial: { target: 'Interaction.Idle', data: { error: null } },
      states: {
        Idle: {
          on: {
            SelectModeScope: {
              target: 'Interaction.Idle',
              update: 'root',
              data: ({ event, root, state }) => ({
                target: { ...state, error: null },
                update: { ...root, modeScope: event.scope, modeTarget: '' },
              }),
            },
            SelectModeTarget: {
              target: 'Interaction.Idle',
              update: 'root',
              data: ({ event, root, state }) => ({
                target: { ...state, error: null },
                update: { ...root, modeTarget: event.targetId },
              }),
            },
            SelectModeSetting: {
              target: 'Interaction.Idle',
              update: 'root',
              data: ({ event, root, state }) => ({
                target: { ...state, error: null },
                update: { ...root, modeSetting: event.mode },
              }),
            },
            SetInteractionMode: {
              target: 'Interaction.Saving',
              data: ({ event }) => ({
                scope: event.scope,
                targetId: event.targetId,
                mode: event.mode,
              }),
            },
            ClearInteractionMode: {
              target: 'Interaction.Removing',
              data: ({ event }) => ({ scope: event.scope, targetId: event.targetId }),
            },
          },
        },
        Saving: {
          invoke: {
            src: 'setInteractionMode',
            input: ({ state }) => ({
              scope: state.scope,
              targetId: state.targetId,
              mode: state.mode,
            }),
            onDone: {
              target: 'Interaction.Idle',
              update: 'root',
              data: ({ root, state, output }) => ({
                target: { error: null },
                update: {
                  ...root,
                  discord: { ...root.discord, interactionModes: output.interactionModes },
                  // Drop the draft only when the saved row was the draft.
                  modeTarget:
                    state.scope === root.modeScope && state.targetId === root.modeTarget
                      ? ''
                      : root.modeTarget,
                },
              }),
            },
            onFailure: {
              target: 'Interaction.Idle',
              data: ({ error }) => ({ error: error.message }),
            },
          },
        },
        Removing: {
          invoke: {
            src: 'clearInteractionMode',
            input: ({ state }) => ({ scope: state.scope, targetId: state.targetId }),
            onDone: {
              target: 'Interaction.Idle',
              update: 'root',
              data: ({ root, state, output }) => ({
                target: { error: null },
                update: {
                  ...root,
                  discord: { ...root.discord, interactionModes: output.interactionModes },
                  modeTarget:
                    state.scope === root.modeScope && state.targetId === root.modeTarget
                      ? ''
                      : root.modeTarget,
                },
              }),
            },
            onFailure: {
              target: 'Interaction.Idle',
              data: ({ error }) => ({ error: error.message }),
            },
          },
        },
      },
    },
  },
})
