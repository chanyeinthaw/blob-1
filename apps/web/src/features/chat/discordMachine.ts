import { Api } from '@blob-1/client/api'
import {
  Discord,
  DiscordAccessEffect,
  DiscordAccessScope,
  DiscordDirectory,
  DiscordInteractionMode,
  DiscordInteractionScope,
  DiscordReplyMode,
  DiscordReplyScope,
  GeneralSettings,
  Home,
} from '@blob-1/contracts'
import { Machine } from '@typeonce/effect-machine'
import { Effect, Schedule, Schema } from 'effect'

const emptyDirectory: DiscordDirectory = { guilds: [], channels: [], people: [] }

const fetchDiscordAll = Effect.gen(function* () {
  const api = yield* Api
  return yield* Effect.all(
    {
      discord: api.request('discord.get', {}),
      directory: api.request('discord.directory', {}),
      general: api.request('general.get', {}),
    },
    { concurrency: 'unbounded' },
  ).pipe(Effect.orElseSucceed(() => null))
})

const saveToken = Effect.fn('DiscordSettings.saveToken')(function* (token: string) {
  const api = yield* Api
  return yield* api.request('discord.setToken', { token })
})

const removeToken = Effect.flatMap(Api, api => api.request('discord.clearToken', {}))

const setDefaultAccess = Effect.fn('DiscordSettings.setDefaultAccess')(function* (
  effect: DiscordAccessEffect,
) {
  const api = yield* Api
  return yield* api.request('discord.setDefaultAccess', { effect })
})

const setAccessRule = Effect.fn('DiscordSettings.setAccessRule')(function* (input: {
  readonly userId: string | null
  readonly scope: DiscordAccessScope
  readonly scopeId: string | null
  readonly effect: DiscordAccessEffect
  readonly refreshDirectory: boolean
}) {
  const api = yield* Api
  const discord = yield* api.request('discord.setAccessRule', {
    userId: input.userId,
    scope: input.scope,
    scopeId: input.scopeId,
    effect: input.effect,
  })
  const directory =
    input.refreshDirectory && discord.connection.state === 'online'
      ? yield* api.request('discord.directory', {}).pipe(Effect.orElseSucceed(() => null))
      : null
  return { discord, directory }
})

const removeAccessRule = Effect.fn('DiscordSettings.removeAccessRule')(function* (input: {
  readonly userId: string | null
  readonly scope: DiscordAccessScope
  readonly scopeId: string | null
}) {
  const api = yield* Api
  return yield* api.request('discord.removeAccessRule', input)
})

const setHome = Effect.fn('DiscordSettings.setHome')(function* (home: Home) {
  const api = yield* Api
  return yield* api.request('discord.setHome', { home })
})

const setInteractionMode = Effect.fn('DiscordSettings.setInteractionMode')(function* (input: {
  readonly scope: DiscordInteractionScope
  readonly guildId: string | null
  readonly targetId: string
  readonly mode: DiscordInteractionMode
}) {
  const api = yield* Api
  return yield* api.request('discord.setInteractionMode', input)
})

const clearInteractionMode = Effect.fn('DiscordSettings.clearInteractionMode')(function* (input: {
  readonly scope: DiscordInteractionScope
  readonly guildId: string | null
  readonly targetId: string
}) {
  const api = yield* Api
  return yield* api.request('discord.clearInteractionMode', input)
})

const setDefaultInteractionMode = Effect.fn('DiscordSettings.setDefaultInteractionMode')(function* (
  mode: DiscordInteractionMode,
) {
  const api = yield* Api
  return yield* api.request('discord.setDefaultInteractionMode', { mode })
})

const setDefaultReplyMode = Effect.fn('DiscordSettings.setDefaultReplyMode')(function* (
  mode: DiscordReplyMode,
) {
  const api = yield* Api
  return yield* api.request('discord.setDefaultReplyMode', { mode })
})

const setReplyMode = Effect.fn('DiscordSettings.setReplyMode')(function* (input: {
  readonly scope: DiscordReplyScope
  readonly targetId: string
  readonly mode: DiscordReplyMode
}) {
  const api = yield* Api
  return yield* api.request('discord.setReplyMode', input)
})

const clearReplyMode = Effect.fn('DiscordSettings.clearReplyMode')(function* (input: {
  readonly scope: DiscordReplyScope
  readonly targetId: string
}) {
  const api = yield* Api
  return yield* api.request('discord.clearReplyMode', input)
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

/** Merges only newly resolved channels into the cached directory so a transient
 *  directory fetch failure never clears existing guilds or people. */
const mergeDirectoryChannels = (
  current: DiscordDirectory,
  refreshed: DiscordDirectory | null,
): DiscordDirectory => {
  if (refreshed === null) return current
  const extra = refreshed.channels.filter(
    channel => !current.channels.some(existing => existing.id === channel.id),
  )
  return extra.length === 0 ? current : { ...current, channels: [...current.channels, ...extra] }
}

export const DiscordInput = Schema.Struct({
  discord: Discord,
  directory: DiscordDirectory,
  general: GeneralSettings,
})
export type DiscordInput = typeof DiscordInput.Type

const ErrorMessage = Schema.NullOr(Schema.String)
const AccessTargetKind = Schema.Literals(['person', 'channel', 'server'])
const HomeKind = Schema.Literals(['none', 'channel', 'dm'])

export const DiscordStates = Machine.state({
  type: 'parallel',
  fields: {
    discord: Discord,
    directory: DiscordDirectory,
    general: GeneralSettings,
    serverDefaultAccess: DiscordAccessEffect,
    pendingDefaultAccess: Schema.NullOr(DiscordAccessEffect),
    inFlightDefaultAccess: Schema.NullOr(DiscordAccessEffect),
    inFlightWrites: Schema.Finite,
    mutationSeq: Schema.Finite,
    /** Bumped on every token save or removal; a connection watch started earlier is stale. */
    tokenGeneration: Schema.Finite,
    /** Transient home type chosen before picking a destination; null follows `discord.home.kind`. */
    pendingHomeKind: Schema.NullOr(HomeKind),
    /** Unified Access add-rule dialog + form draft. */
    accessDialogOpen: Schema.Boolean,
    accessKind: AccessTargetKind,
    accessScope: DiscordAccessScope,
    accessScopeId: Schema.String,
    accessUser: Schema.String,
    accessEffect: DiscordAccessEffect,
    /** Interaction mode dialog + form draft; root fields so busy states can't lose it. */
    modeDialogOpen: Schema.Boolean,
    modeScope: DiscordInteractionScope,
    modeGuild: Schema.String,
    modeTarget: Schema.String,
    modeSetting: DiscordInteractionMode,
    /** Reply-mode dialog + form draft. */
    replyDialogOpen: Schema.Boolean,
    replyScope: DiscordReplyScope,
    replyTarget: Schema.String,
    replySetting: DiscordReplyMode,
  },
  states: {
    Sync: {
      states: {
        Idle: {},
        Refreshing: { fields: { mutationSeq: Schema.Finite } },
      },
    },
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
    Access: {
      states: {
        Idle: { fields: { error: ErrorMessage } },
        SettingDefault: { fields: { effect: DiscordAccessEffect } },
        SavingRule: {
          fields: {
            userId: Schema.NullOr(Schema.String),
            scope: DiscordAccessScope,
            scopeId: Schema.NullOr(Schema.String),
            effect: DiscordAccessEffect,
            refreshDirectory: Schema.Boolean,
            tokenGeneration: Schema.Finite,
            closeDialog: Schema.Boolean,
          },
        },
        RemovingRule: {
          fields: {
            userId: Schema.NullOr(Schema.String),
            scope: DiscordAccessScope,
            scopeId: Schema.NullOr(Schema.String),
          },
        },
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
            guildId: Schema.NullOr(Schema.String),
            targetId: Schema.String,
            mode: DiscordInteractionMode,
          },
        },
        SettingDefault: {
          fields: { mode: DiscordInteractionMode, previous: DiscordInteractionMode },
        },
        Removing: {
          fields: {
            scope: DiscordInteractionScope,
            guildId: Schema.NullOr(Schema.String),
            targetId: Schema.String,
          },
        },
      },
    },
    ReplyMode: {
      states: {
        Idle: { fields: { error: ErrorMessage } },
        Saving: {
          fields: {
            scope: DiscordReplyScope,
            targetId: Schema.String,
            mode: DiscordReplyMode,
          },
        },
        SettingDefault: {
          fields: { mode: DiscordReplyMode, previous: DiscordReplyMode },
        },
        Removing: {
          fields: {
            scope: DiscordReplyScope,
            targetId: Schema.String,
          },
        },
      },
    },
  },
})

export const DiscordEvents = Machine.events({
  Refresh: {},
  ReplaceToken: {},
  CancelToken: {},
  ClearTokenError: {},
  SubmitToken: { token: Schema.String },
  RemoveToken: {},
  OpenAccessDialog: {},
  CloseAccessDialog: {},
  SelectAccessKind: { kind: AccessTargetKind },
  SelectAccessScope: { scope: DiscordAccessScope },
  SelectAccessScopeId: { scopeId: Schema.String },
  SelectAccessUser: { userId: Schema.String },
  SelectAccessEffect: { effect: DiscordAccessEffect },
  SetDefaultAccess: { effect: DiscordAccessEffect },
  SetAccessRule: {
    userId: Schema.NullOr(Schema.String),
    scope: DiscordAccessScope,
    scopeId: Schema.NullOr(Schema.String),
    effect: DiscordAccessEffect,
    closeDialog: Schema.Boolean,
  },
  RemoveAccessRule: {
    userId: Schema.NullOr(Schema.String),
    scope: DiscordAccessScope,
    scopeId: Schema.NullOr(Schema.String),
  },
  SelectHomeKind: { kind: HomeKind },
  SetHome: { home: Home },
  OpenModeDialog: {},
  CloseModeDialog: {},
  SelectModeScope: { scope: DiscordInteractionScope },
  SelectModeGuild: { guildId: Schema.String },
  SelectModeTarget: { targetId: Schema.String },
  SelectModeSetting: { mode: DiscordInteractionMode },
  SetInteractionMode: {
    scope: DiscordInteractionScope,
    guildId: Schema.NullOr(Schema.String),
    targetId: Schema.String,
    mode: DiscordInteractionMode,
  },
  SetDefaultInteractionMode: { mode: DiscordInteractionMode },
  ClearInteractionMode: {
    scope: DiscordInteractionScope,
    guildId: Schema.NullOr(Schema.String),
    targetId: Schema.String,
  },
  OpenReplyDialog: {},
  CloseReplyDialog: {},
  SelectReplyScope: { scope: DiscordReplyScope },
  SelectReplyTarget: { targetId: Schema.String },
  SelectReplySetting: { mode: DiscordReplyMode },
  SetReplyMode: {
    scope: DiscordReplyScope,
    targetId: Schema.String,
    mode: DiscordReplyMode,
  },
  SetDefaultReplyMode: { mode: DiscordReplyMode },
  ClearReplyMode: { scope: DiscordReplyScope, targetId: Schema.String },
})

/** The Discord settings page: token, live connection status, allowlist, overrides and home channel. */
export const DiscordMachine = Machine.make({
  id: 'DiscordSettings',
  root: DiscordStates,
  events: DiscordEvents,
  input: DiscordInput,
  effects: {
    fetchDiscordAll,
    saveToken,
    removeToken,
    watchConnection,
    setDefaultAccess,
    setAccessRule,
    removeAccessRule,
    setHome,
    setInteractionMode,
    clearInteractionMode,
    setDefaultInteractionMode,
    setReplyMode,
    clearReplyMode,
    setDefaultReplyMode,
  },
}).handle({
  root: ({ input }) => ({
    ...input,
    serverDefaultAccess: input.discord.defaultAccess,
    pendingDefaultAccess: null,
    inFlightDefaultAccess: null,
    inFlightWrites: 0,
    mutationSeq: 0,
    tokenGeneration: 0,
    pendingHomeKind: null,
    accessDialogOpen: false,
    accessKind: 'person',
    accessScope: 'global',
    accessScopeId: '',
    accessUser: '',
    accessEffect: input.discord.defaultAccess === 'block' ? 'allow' : 'block',
    modeDialogOpen: false,
    modeScope: 'channel',
    modeGuild: input.directory.guilds.length === 1 ? (input.directory.guilds[0]?.id ?? '') : '',
    modeTarget: '',
    modeSetting: 'always',
    replyDialogOpen: false,
    replyScope: 'channel',
    replyTarget: '',
    replySetting: 'direct',
  }),
  on: {
    SetDefaultAccess: {
      update: 'root',
      data: ({ root, event }) => ({
        ...root,
        pendingDefaultAccess:
          root.inFlightDefaultAccess === null && event.effect === root.serverDefaultAccess
            ? null
            : event.effect,
        mutationSeq: root.mutationSeq + 1,
        discord: { ...root.discord, defaultAccess: event.effect },
      }),
    },
  },
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
            src: 'fetchDiscordAll',
            onDone: {
              target: 'Sync.Idle',
              update: 'root',
              data: ({ root, state, output }) => {
                if (
                  output === null ||
                  state.mutationSeq !== root.mutationSeq ||
                  root.inFlightWrites > 0 ||
                  root.inFlightDefaultAccess !== null ||
                  root.pendingDefaultAccess !== null
                ) {
                  return { target: undefined, update: root }
                }
                return {
                  target: undefined,
                  update: {
                    ...root,
                    serverDefaultAccess: output.discord.defaultAccess,
                    discord: output.discord,
                    general: output.general,
                    directory:
                      output.directory.guilds.length > 0 ||
                      output.directory.channels.length > 0 ||
                      output.directory.people.length > 0
                        ? output.directory
                        : root.directory,
                  },
                }
              },
            },
          },
        },
      },
    },
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
            RemoveToken: {
              target: 'Token.Removing',
              update: 'root',
              data: ({ root }) => ({
                target: undefined,
                update: {
                  ...root,
                  inFlightWrites: root.inFlightWrites + 1,
                  mutationSeq: root.mutationSeq + 1,
                },
              }),
            },
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
              update: 'root',
              data: ({ event, root }) => ({
                target: { token: event.token.trim() },
                update: {
                  ...root,
                  inFlightWrites: root.inFlightWrites + 1,
                  mutationSeq: root.mutationSeq + 1,
                },
              }),
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
                    ? {
                        ...root,
                        inFlightWrites: Math.max(0, root.inFlightWrites - 1),
                        mutationSeq: root.mutationSeq + 1,
                        discord: withToken(root.discord, output),
                      }
                    : {
                        ...root,
                        inFlightWrites: Math.max(0, root.inFlightWrites - 1),
                        mutationSeq: root.mutationSeq + 1,
                        discord: withToken(root.discord, output),
                        directory: emptyDirectory,
                        accessScopeId: '',
                        accessUser: '',
                        modeGuild: '',
                        modeTarget: '',
                        replyTarget: '',
                        tokenGeneration: root.tokenGeneration + 1,
                      },
              }),
            },
            onFailure: {
              target: 'Token.Editing',
              update: 'root',
              data: ({ root, error }) => ({
                target: { error: error.message },
                update: {
                  ...root,
                  inFlightWrites: Math.max(0, root.inFlightWrites - 1),
                  mutationSeq: root.mutationSeq + 1,
                },
              }),
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
                  inFlightWrites: Math.max(0, root.inFlightWrites - 1),
                  mutationSeq: root.mutationSeq + 1,
                  discord: withToken(root.discord, output),
                  directory: emptyDirectory,
                  accessScopeId: '',
                  accessUser: '',
                  modeGuild: '',
                  modeTarget: '',
                  replyTarget: '',
                  tokenGeneration: root.tokenGeneration + 1,
                },
              }),
            },
            onFailure: {
              target: 'Token.Editing',
              update: 'root',
              data: ({ root, error }) => ({
                target: { error: error.message },
                update: {
                  ...root,
                  inFlightWrites: Math.max(0, root.inFlightWrites - 1),
                  mutationSeq: root.mutationSeq + 1,
                },
              }),
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
                        directory: mergeDirectoryChannels(output.directory, root.directory),
                        accessScopeId:
                          root.accessScope === 'server' &&
                          root.accessScopeId === '' &&
                          output.directory.guilds.length === 1
                            ? (output.directory.guilds[0]?.id ?? '')
                            : root.accessScopeId,
                        modeGuild:
                          root.modeGuild === '' && output.directory.guilds.length === 1
                            ? (output.directory.guilds[0]?.id ?? '')
                            : root.modeGuild,
                      }
                    : root,
              }),
            },
            onFailure: { target: 'Status.Settled' },
          },
        },
      },
    },
    Access: {
      initial: { target: 'Access.Idle', data: { error: null } },
      states: {
        Idle: {
          always: {
            guard: ({ root }) =>
              root.pendingDefaultAccess !== null &&
              root.pendingDefaultAccess !== root.serverDefaultAccess,
            target: 'Access.SettingDefault',
            update: 'root',
            data: ({ root }) => {
              const effect = root.pendingDefaultAccess ?? root.serverDefaultAccess
              return {
                target: { effect },
                update: {
                  ...root,
                  pendingDefaultAccess: null,
                  inFlightDefaultAccess: effect,
                  mutationSeq: root.mutationSeq + 1,
                },
              }
            },
          },
          on: {
            OpenAccessDialog: {
              target: 'Access.Idle',
              update: 'root',
              data: ({ root }) => ({
                target: { error: null },
                update: {
                  ...root,
                  accessDialogOpen: true,
                  accessKind: 'person',
                  accessScope: 'global',
                  accessScopeId: '',
                  accessUser: '',
                  accessEffect: root.discord.defaultAccess === 'block' ? 'allow' : 'block',
                },
              }),
            },
            CloseAccessDialog: {
              target: 'Access.Idle',
              update: 'root',
              data: ({ root }) => ({
                target: { error: null },
                update: { ...root, accessDialogOpen: false, accessUser: '', accessScopeId: '' },
              }),
            },
            SelectAccessKind: {
              target: 'Access.Idle',
              update: 'root',
              data: ({ event, root, state }) => ({
                target: { ...state, error: null },
                update: {
                  ...root,
                  accessKind: event.kind,
                  accessScope:
                    event.kind === 'person'
                      ? 'global'
                      : event.kind === 'server'
                        ? 'server'
                        : 'channel',
                  accessScopeId:
                    event.kind === 'server' && root.directory.guilds.length === 1
                      ? (root.directory.guilds[0]?.id ?? '')
                      : '',
                  accessUser: '',
                },
              }),
            },
            SelectAccessScope: {
              target: 'Access.Idle',
              update: 'root',
              data: ({ event, root, state }) => ({
                target: { ...state, error: null },
                update: {
                  ...root,
                  accessScope: event.scope,
                  accessScopeId:
                    event.scope === 'server' && root.directory.guilds.length === 1
                      ? (root.directory.guilds[0]?.id ?? '')
                      : '',
                  accessUser: '',
                },
              }),
            },
            SelectAccessScopeId: {
              target: 'Access.Idle',
              update: 'root',
              data: ({ event, root, state }) => ({
                target: { ...state, error: null },
                update: { ...root, accessScopeId: event.scopeId, accessUser: '' },
              }),
            },
            SelectAccessUser: {
              target: 'Access.Idle',
              update: 'root',
              data: ({ event, root, state }) => ({
                target: { ...state, error: null },
                update: { ...root, accessUser: event.userId },
              }),
            },
            SelectAccessEffect: {
              target: 'Access.Idle',
              update: 'root',
              data: ({ event, root, state }) => ({
                target: { ...state, error: null },
                update: { ...root, accessEffect: event.effect },
              }),
            },
            SetAccessRule: {
              target: 'Access.SavingRule',
              update: 'root',
              data: ({ event, root }) => ({
                target: {
                  userId: event.userId,
                  scope: event.scope,
                  scopeId: event.scopeId,
                  effect: event.effect,
                  refreshDirectory:
                    event.scope === 'channel' &&
                    event.scopeId !== null &&
                    !root.directory.channels.some(channel => channel.id === event.scopeId),
                  tokenGeneration: root.tokenGeneration,
                  closeDialog: event.closeDialog,
                },
                update: {
                  ...root,
                  inFlightWrites: root.inFlightWrites + 1,
                  mutationSeq: root.mutationSeq + 1,
                },
              }),
            },
            RemoveAccessRule: {
              target: 'Access.RemovingRule',
              update: 'root',
              data: ({ event, root }) => ({
                target: {
                  userId: event.userId,
                  scope: event.scope,
                  scopeId: event.scopeId,
                },
                update: {
                  ...root,
                  inFlightWrites: root.inFlightWrites + 1,
                  mutationSeq: root.mutationSeq + 1,
                },
              }),
            },
          },
        },
        SettingDefault: {
          invoke: {
            src: 'setDefaultAccess',
            input: ({ state }) => state.effect,
            onDone: {
              target: 'Access.Idle',
              update: 'root',
              data: ({ root, output }) => {
                const nextPending =
                  root.pendingDefaultAccess === output.defaultAccess
                    ? null
                    : root.pendingDefaultAccess
                return {
                  target: { error: null },
                  update: {
                    ...root,
                    serverDefaultAccess: output.defaultAccess,
                    pendingDefaultAccess: nextPending,
                    inFlightDefaultAccess: null,
                    mutationSeq: root.mutationSeq + 1,
                    discord: {
                      ...root.discord,
                      allowed: output.allowed,
                      defaultAccess: nextPending ?? output.defaultAccess,
                      accessRules: output.accessRules,
                    },
                  },
                }
              },
            },
            onFailure: {
              target: 'Access.Idle',
              update: 'root',
              data: ({ root, state, error }) => {
                const nextPending =
                  root.pendingDefaultAccess === state.effect ||
                  root.pendingDefaultAccess === root.serverDefaultAccess
                    ? null
                    : root.pendingDefaultAccess
                return {
                  target: { error: error.message },
                  update: {
                    ...root,
                    pendingDefaultAccess: nextPending,
                    inFlightDefaultAccess: null,
                    mutationSeq: root.mutationSeq + 1,
                    discord: {
                      ...root.discord,
                      defaultAccess: nextPending ?? root.serverDefaultAccess,
                    },
                  },
                }
              },
            },
          },
        },
        SavingRule: {
          invoke: {
            src: 'setAccessRule',
            input: ({ state }) => ({
              userId: state.userId,
              scope: state.scope,
              scopeId: state.scopeId,
              effect: state.effect,
              refreshDirectory: state.refreshDirectory,
            }),
            onDone: {
              target: 'Access.Idle',
              update: 'root',
              data: ({ root, state, output }) => {
                const nextPending =
                  root.pendingDefaultAccess === output.discord.defaultAccess
                    ? null
                    : root.pendingDefaultAccess
                return {
                  target: { error: null },
                  update: {
                    ...root,
                    inFlightWrites: Math.max(0, root.inFlightWrites - 1),
                    serverDefaultAccess: output.discord.defaultAccess,
                    pendingDefaultAccess: nextPending,
                    mutationSeq: root.mutationSeq + 1,
                    discord: {
                      ...root.discord,
                      allowed: output.discord.allowed,
                      defaultAccess: nextPending ?? output.discord.defaultAccess,
                      accessRules: output.discord.accessRules,
                    },
                    directory:
                      state.tokenGeneration === root.tokenGeneration
                        ? mergeDirectoryChannels(root.directory, output.directory)
                        : root.directory,
                    accessDialogOpen: state.closeDialog ? false : root.accessDialogOpen,
                    accessUser: state.closeDialog ? '' : root.accessUser,
                    accessScopeId: state.closeDialog ? '' : root.accessScopeId,
                  },
                }
              },
            },
            onFailure: {
              target: 'Access.Idle',
              update: 'root',
              data: ({ root, error }) => ({
                target: { error: error.message },
                update: {
                  ...root,
                  inFlightWrites: Math.max(0, root.inFlightWrites - 1),
                  mutationSeq: root.mutationSeq + 1,
                },
              }),
            },
          },
        },
        RemovingRule: {
          invoke: {
            src: 'removeAccessRule',
            input: ({ state }) => ({
              userId: state.userId,
              scope: state.scope,
              scopeId: state.scopeId,
            }),
            onDone: {
              target: 'Access.Idle',
              update: 'root',
              data: ({ root, output }) => {
                const nextPending =
                  root.pendingDefaultAccess === output.defaultAccess
                    ? null
                    : root.pendingDefaultAccess
                return {
                  target: { error: null },
                  update: {
                    ...root,
                    inFlightWrites: Math.max(0, root.inFlightWrites - 1),
                    serverDefaultAccess: output.defaultAccess,
                    pendingDefaultAccess: nextPending,
                    mutationSeq: root.mutationSeq + 1,
                    discord: {
                      ...root.discord,
                      allowed: output.allowed,
                      defaultAccess: nextPending ?? output.defaultAccess,
                      accessRules: output.accessRules,
                    },
                  },
                }
              },
            },
            onFailure: {
              target: 'Access.Idle',
              update: 'root',
              data: ({ root, error }) => ({
                target: { error: error.message },
                update: {
                  ...root,
                  inFlightWrites: Math.max(0, root.inFlightWrites - 1),
                  mutationSeq: root.mutationSeq + 1,
                },
              }),
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
            update: 'root',
            data: ({ state, root }) => ({
              target: { home: state.queued ?? { kind: 'none' }, queued: null },
              update: {
                ...root,
                inFlightWrites: root.inFlightWrites + 1,
                mutationSeq: root.mutationSeq + 1,
              },
            }),
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
              update: 'root',
              data: ({ event, root }) => ({
                target: { home: event.home, queued: null },
                update: {
                  ...root,
                  inFlightWrites: root.inFlightWrites + 1,
                  mutationSeq: root.mutationSeq + 1,
                },
              }),
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
                  inFlightWrites: Math.max(0, root.inFlightWrites - 1),
                  mutationSeq: root.mutationSeq + 1,
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
                  inFlightWrites: Math.max(0, root.inFlightWrites - 1),
                  mutationSeq: root.mutationSeq + 1,
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
            OpenModeDialog: {
              target: 'Interaction.Idle',
              update: 'root',
              data: ({ root }) => ({
                target: { error: null },
                update: {
                  ...root,
                  modeDialogOpen: true,
                  modeScope: 'channel',
                  modeGuild:
                    root.directory.guilds.length === 1 ? (root.directory.guilds[0]?.id ?? '') : '',
                  modeTarget: '',
                  modeSetting: 'always',
                },
              }),
            },
            CloseModeDialog: {
              target: 'Interaction.Idle',
              update: 'root',
              data: ({ root }) => ({
                target: { error: null },
                update: { ...root, modeDialogOpen: false, modeTarget: '' },
              }),
            },
            SelectModeScope: {
              target: 'Interaction.Idle',
              update: 'root',
              data: ({ event, root, state }) => ({
                target: { ...state, error: null },
                update: {
                  ...root,
                  modeScope: event.scope,
                  modeGuild:
                    event.scope === 'user' &&
                    root.modeGuild === '' &&
                    root.directory.guilds.length === 1
                      ? (root.directory.guilds[0]?.id ?? '')
                      : root.modeGuild,
                  modeTarget: '',
                },
              }),
            },
            SelectModeGuild: {
              target: 'Interaction.Idle',
              update: 'root',
              data: ({ event, root, state }) => ({
                target: { ...state, error: null },
                update: { ...root, modeGuild: event.guildId, modeTarget: '' },
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
              update: 'root',
              data: ({ event, root }) => ({
                target: {
                  scope: event.scope,
                  guildId: event.guildId,
                  targetId: event.targetId,
                  mode: event.mode,
                },
                update: {
                  ...root,
                  inFlightWrites: root.inFlightWrites + 1,
                  mutationSeq: root.mutationSeq + 1,
                },
              }),
            },
            SetDefaultInteractionMode: {
              target: 'Interaction.SettingDefault',
              update: 'root',
              data: ({ event, root }) => ({
                target: { mode: event.mode, previous: root.discord.defaultInteractionMode },
                update: {
                  ...root,
                  inFlightWrites: root.inFlightWrites + 1,
                  mutationSeq: root.mutationSeq + 1,
                  discord: { ...root.discord, defaultInteractionMode: event.mode },
                },
              }),
            },
            ClearInteractionMode: {
              target: 'Interaction.Removing',
              update: 'root',
              data: ({ event, root }) => ({
                target: {
                  scope: event.scope,
                  guildId: event.guildId,
                  targetId: event.targetId,
                },
                update: {
                  ...root,
                  inFlightWrites: root.inFlightWrites + 1,
                  mutationSeq: root.mutationSeq + 1,
                },
              }),
            },
          },
        },
        Saving: {
          invoke: {
            src: 'setInteractionMode',
            input: ({ state }) => ({
              scope: state.scope,
              guildId: state.guildId,
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
                  inFlightWrites: Math.max(0, root.inFlightWrites - 1),
                  mutationSeq: root.mutationSeq + 1,
                  discord: { ...root.discord, interactionModes: output.interactionModes },
                  modeDialogOpen: false,
                  // Drop the draft only when the saved row was the draft.
                  modeTarget:
                    state.scope === root.modeScope &&
                    (state.scope !== 'user' || state.guildId === root.modeGuild) &&
                    state.targetId === root.modeTarget
                      ? ''
                      : root.modeTarget,
                },
              }),
            },
            onFailure: {
              target: 'Interaction.Idle',
              update: 'root',
              data: ({ root, error }) => ({
                target: { error: error.message },
                update: {
                  ...root,
                  inFlightWrites: Math.max(0, root.inFlightWrites - 1),
                  mutationSeq: root.mutationSeq + 1,
                },
              }),
            },
          },
        },
        SettingDefault: {
          invoke: {
            src: 'setDefaultInteractionMode',
            input: ({ state }) => state.mode,
            onDone: {
              target: 'Interaction.Idle',
              update: 'root',
              data: ({ root, output }) => ({
                target: { error: null },
                update: {
                  ...root,
                  inFlightWrites: Math.max(0, root.inFlightWrites - 1),
                  mutationSeq: root.mutationSeq + 1,
                  discord: {
                    ...root.discord,
                    defaultInteractionMode: output.defaultInteractionMode,
                  },
                },
              }),
            },
            onFailure: {
              target: 'Interaction.Idle',
              update: 'root',
              data: ({ root, state, error }) => ({
                target: { error: error.message },
                update: {
                  ...root,
                  inFlightWrites: Math.max(0, root.inFlightWrites - 1),
                  mutationSeq: root.mutationSeq + 1,
                  discord: { ...root.discord, defaultInteractionMode: state.previous },
                },
              }),
            },
          },
        },
        Removing: {
          invoke: {
            src: 'clearInteractionMode',
            input: ({ state }) => ({
              scope: state.scope,
              guildId: state.guildId,
              targetId: state.targetId,
            }),
            onDone: {
              target: 'Interaction.Idle',
              update: 'root',
              data: ({ root, state, output }) => ({
                target: { error: null },
                update: {
                  ...root,
                  inFlightWrites: Math.max(0, root.inFlightWrites - 1),
                  mutationSeq: root.mutationSeq + 1,
                  discord: { ...root.discord, interactionModes: output.interactionModes },
                  modeTarget:
                    state.scope === root.modeScope &&
                    (state.scope !== 'user' || state.guildId === root.modeGuild) &&
                    state.targetId === root.modeTarget
                      ? ''
                      : root.modeTarget,
                },
              }),
            },
            onFailure: {
              target: 'Interaction.Idle',
              update: 'root',
              data: ({ root, error }) => ({
                target: { error: error.message },
                update: {
                  ...root,
                  inFlightWrites: Math.max(0, root.inFlightWrites - 1),
                  mutationSeq: root.mutationSeq + 1,
                },
              }),
            },
          },
        },
      },
    },
    ReplyMode: {
      initial: { target: 'ReplyMode.Idle', data: { error: null } },
      states: {
        Idle: {
          on: {
            OpenReplyDialog: {
              target: 'ReplyMode.Idle',
              update: 'root',
              data: ({ root }) => ({
                target: { error: null },
                update: {
                  ...root,
                  replyDialogOpen: true,
                  replyScope: 'channel',
                  replyTarget: '',
                  replySetting: 'direct',
                },
              }),
            },
            CloseReplyDialog: {
              target: 'ReplyMode.Idle',
              update: 'root',
              data: ({ root }) => ({
                target: { error: null },
                update: { ...root, replyDialogOpen: false, replyTarget: '' },
              }),
            },
            SelectReplyScope: {
              target: 'ReplyMode.Idle',
              update: 'root',
              data: ({ event, root, state }) => ({
                target: { ...state, error: null },
                update: { ...root, replyScope: event.scope, replyTarget: '' },
              }),
            },
            SelectReplyTarget: {
              target: 'ReplyMode.Idle',
              update: 'root',
              data: ({ event, root, state }) => ({
                target: { ...state, error: null },
                update: { ...root, replyTarget: event.targetId },
              }),
            },
            SelectReplySetting: {
              target: 'ReplyMode.Idle',
              update: 'root',
              data: ({ event, root, state }) => ({
                target: { ...state, error: null },
                update: { ...root, replySetting: event.mode },
              }),
            },
            SetReplyMode: {
              target: 'ReplyMode.Saving',
              update: 'root',
              data: ({ event, root }) => ({
                target: {
                  scope: event.scope,
                  targetId: event.targetId,
                  mode: event.mode,
                },
                update: {
                  ...root,
                  inFlightWrites: root.inFlightWrites + 1,
                  mutationSeq: root.mutationSeq + 1,
                },
              }),
            },
            SetDefaultReplyMode: {
              target: 'ReplyMode.SettingDefault',
              update: 'root',
              data: ({ event, root }) => ({
                target: { mode: event.mode, previous: root.discord.defaultReplyMode },
                update: {
                  ...root,
                  inFlightWrites: root.inFlightWrites + 1,
                  mutationSeq: root.mutationSeq + 1,
                  discord: { ...root.discord, defaultReplyMode: event.mode },
                },
              }),
            },
            ClearReplyMode: {
              target: 'ReplyMode.Removing',
              update: 'root',
              data: ({ event, root }) => ({
                target: { scope: event.scope, targetId: event.targetId },
                update: {
                  ...root,
                  inFlightWrites: root.inFlightWrites + 1,
                  mutationSeq: root.mutationSeq + 1,
                },
              }),
            },
          },
        },
        Saving: {
          invoke: {
            src: 'setReplyMode',
            input: ({ state }) => ({
              scope: state.scope,
              targetId: state.targetId,
              mode: state.mode,
            }),
            onDone: {
              target: 'ReplyMode.Idle',
              update: 'root',
              data: ({ root, state, output }) => ({
                target: { error: null },
                update: {
                  ...root,
                  inFlightWrites: Math.max(0, root.inFlightWrites - 1),
                  mutationSeq: root.mutationSeq + 1,
                  discord: { ...root.discord, replyModes: output.replyModes },
                  replyDialogOpen: false,
                  // Drop the draft only when the saved row was the draft.
                  replyTarget:
                    state.scope === root.replyScope && state.targetId === root.replyTarget
                      ? ''
                      : root.replyTarget,
                },
              }),
            },
            onFailure: {
              target: 'ReplyMode.Idle',
              update: 'root',
              data: ({ root, error }) => ({
                target: { error: error.message },
                update: {
                  ...root,
                  inFlightWrites: Math.max(0, root.inFlightWrites - 1),
                  mutationSeq: root.mutationSeq + 1,
                },
              }),
            },
          },
        },
        SettingDefault: {
          invoke: {
            src: 'setDefaultReplyMode',
            input: ({ state }) => state.mode,
            onDone: {
              target: 'ReplyMode.Idle',
              update: 'root',
              data: ({ root, output }) => ({
                target: { error: null },
                update: {
                  ...root,
                  inFlightWrites: Math.max(0, root.inFlightWrites - 1),
                  mutationSeq: root.mutationSeq + 1,
                  discord: { ...root.discord, defaultReplyMode: output.defaultReplyMode },
                },
              }),
            },
            onFailure: {
              target: 'ReplyMode.Idle',
              update: 'root',
              data: ({ root, state, error }) => ({
                target: { error: error.message },
                update: {
                  ...root,
                  inFlightWrites: Math.max(0, root.inFlightWrites - 1),
                  mutationSeq: root.mutationSeq + 1,
                  discord: { ...root.discord, defaultReplyMode: state.previous },
                },
              }),
            },
          },
        },
        Removing: {
          invoke: {
            src: 'clearReplyMode',
            input: ({ state }) => ({ scope: state.scope, targetId: state.targetId }),
            onDone: {
              target: 'ReplyMode.Idle',
              update: 'root',
              data: ({ root, state, output }) => ({
                target: { error: null },
                update: {
                  ...root,
                  inFlightWrites: Math.max(0, root.inFlightWrites - 1),
                  mutationSeq: root.mutationSeq + 1,
                  discord: { ...root.discord, replyModes: output.replyModes },
                  replyTarget:
                    state.scope === root.replyScope && state.targetId === root.replyTarget
                      ? ''
                      : root.replyTarget,
                },
              }),
            },
            onFailure: {
              target: 'ReplyMode.Idle',
              update: 'root',
              data: ({ root, error }) => ({
                target: { error: error.message },
                update: {
                  ...root,
                  inFlightWrites: Math.max(0, root.inFlightWrites - 1),
                  mutationSeq: root.mutationSeq + 1,
                },
              }),
            },
          },
        },
      },
    },
  },
})
