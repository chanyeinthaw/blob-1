import { Api } from '@blob-1/client/api'
import {
  Discord,
  DiscordAccessList,
  DiscordAccessMode,
  DiscordAccessScope,
  DiscordChannelAccessScope,
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

const setAccessMode = Effect.fn('DiscordSettings.setAccessMode')(function* (
  mode: DiscordAccessMode,
) {
  const api = yield* Api
  return yield* api.request('discord.setAccessMode', { mode })
})

const addAccessEntry = Effect.fn('DiscordSettings.addAccessEntry')(function* (input: {
  readonly list: DiscordAccessList
  readonly scope: DiscordAccessScope
  readonly scopeId: string | null
  readonly userId: string
  readonly refreshDirectory: boolean
}) {
  const api = yield* Api
  const discord = yield* api.request('discord.addAccessEntry', {
    list: input.list,
    scope: input.scope,
    scopeId: input.scopeId,
    userId: input.userId,
  })
  const directory =
    input.refreshDirectory && discord.connection.state === 'online'
      ? yield* api.request('discord.directory', {}).pipe(Effect.orElseSucceed(() => null))
      : null
  return { discord, directory }
})

const removeAccessEntry = Effect.fn('DiscordSettings.removeAccessEntry')(function* (input: {
  readonly list: DiscordAccessList
  readonly scope: DiscordAccessScope
  readonly scopeId: string | null
  readonly userId: string
}) {
  const api = yield* Api
  return yield* api.request('discord.removeAccessEntry', input)
})

const setChannelAccessMode = Effect.fn('DiscordSettings.setChannelAccessMode')(function* (
  mode: DiscordAccessMode,
) {
  const api = yield* Api
  return yield* api.request('discord.setChannelAccessMode', { mode })
})

const addChannelAccessEntry = Effect.fn('DiscordSettings.addChannelAccessEntry')(function* (input: {
  readonly list: DiscordAccessList
  readonly scope: DiscordChannelAccessScope
  readonly targetId: string
  readonly refreshDirectory: boolean
}) {
  const api = yield* Api
  const discord = yield* api.request('discord.addChannelAccessEntry', {
    list: input.list,
    scope: input.scope,
    targetId: input.targetId,
  })
  const directory =
    input.refreshDirectory && discord.connection.state === 'online'
      ? yield* api.request('discord.directory', {}).pipe(Effect.orElseSucceed(() => null))
      : null
  return { discord, directory }
})

const removeChannelAccessEntry = Effect.fn('DiscordSettings.removeChannelAccessEntry')(
  function* (input: {
    readonly list: DiscordAccessList
    readonly scope: DiscordChannelAccessScope
    readonly targetId: string
  }) {
    const api = yield* Api
    return yield* api.request('discord.removeChannelAccessEntry', input)
  },
)

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
const HomeKind = Schema.Literals(['none', 'channel', 'dm'])

export const DiscordStates = Machine.state({
  type: 'parallel',
  fields: {
    discord: Discord,
    directory: DiscordDirectory,
    general: GeneralSettings,
    serverAccessMode: DiscordAccessMode,
    pendingAccessMode: Schema.NullOr(DiscordAccessMode),
    inFlightAccessMode: Schema.NullOr(DiscordAccessMode),
    serverChannelAccessMode: DiscordAccessMode,
    pendingChannelAccessMode: Schema.NullOr(DiscordAccessMode),
    inFlightChannelAccessMode: Schema.NullOr(DiscordAccessMode),
    inFlightWrites: Schema.Finite,
    mutationSeq: Schema.Finite,
    /** Bumped on every token save or removal; a connection watch started earlier is stale. */
    tokenGeneration: Schema.Finite,
    /** Transient home type chosen before picking a destination; null follows `discord.home.kind`. */
    pendingHomeKind: Schema.NullOr(HomeKind),
    /** Access add-entry dialog + form draft. */
    accessDialogOpen: Schema.Boolean,
    accessScope: DiscordAccessScope,
    accessScopeId: Schema.String,
    accessUser: Schema.String,
    /** Channel/server access add-entry dialog + form draft. */
    channelAccessDialogOpen: Schema.Boolean,
    channelAccessScope: DiscordChannelAccessScope,
    channelAccessTarget: Schema.String,
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
        SettingMode: { fields: { mode: DiscordAccessMode } },
        Adding: {
          fields: {
            list: DiscordAccessList,
            scope: DiscordAccessScope,
            scopeId: Schema.NullOr(Schema.String),
            userId: Schema.String,
            refreshDirectory: Schema.Boolean,
            tokenGeneration: Schema.Finite,
          },
        },
        Removing: {
          fields: {
            list: DiscordAccessList,
            scope: DiscordAccessScope,
            scopeId: Schema.NullOr(Schema.String),
            userId: Schema.String,
          },
        },
      },
    },
    ChannelAccess: {
      states: {
        Idle: { fields: { error: ErrorMessage } },
        SettingMode: { fields: { mode: DiscordAccessMode } },
        Adding: {
          fields: {
            list: DiscordAccessList,
            scope: DiscordChannelAccessScope,
            targetId: Schema.String,
            refreshDirectory: Schema.Boolean,
            tokenGeneration: Schema.Finite,
          },
        },
        Removing: {
          fields: {
            list: DiscordAccessList,
            scope: DiscordChannelAccessScope,
            targetId: Schema.String,
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
  SelectAccessScope: { scope: DiscordAccessScope },
  SelectAccessScopeId: { scopeId: Schema.String },
  SelectAccessUser: { userId: Schema.String },
  SetAccessMode: { mode: DiscordAccessMode },
  AddAccessEntry: {
    list: DiscordAccessList,
    scope: DiscordAccessScope,
    scopeId: Schema.NullOr(Schema.String),
    userId: Schema.String,
  },
  RemoveAccessEntry: {
    list: DiscordAccessList,
    scope: DiscordAccessScope,
    scopeId: Schema.NullOr(Schema.String),
    userId: Schema.String,
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
  OpenChannelAccessDialog: {},
  CloseChannelAccessDialog: {},
  SelectChannelAccessScope: { scope: DiscordChannelAccessScope },
  SelectChannelAccessTarget: { targetId: Schema.String },
  SetChannelAccessMode: { mode: DiscordAccessMode },
  AddChannelAccessEntry: {
    list: DiscordAccessList,
    scope: DiscordChannelAccessScope,
    targetId: Schema.String,
  },
  RemoveChannelAccessEntry: {
    list: DiscordAccessList,
    scope: DiscordChannelAccessScope,
    targetId: Schema.String,
  },
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
    setAccessMode,
    addAccessEntry,
    removeAccessEntry,
    setChannelAccessMode,
    addChannelAccessEntry,
    removeChannelAccessEntry,
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
    serverAccessMode: input.discord.accessMode,
    pendingAccessMode: null,
    inFlightAccessMode: null,
    serverChannelAccessMode: input.discord.channelAccessMode,
    pendingChannelAccessMode: null,
    inFlightChannelAccessMode: null,
    inFlightWrites: 0,
    mutationSeq: 0,
    tokenGeneration: 0,
    pendingHomeKind: null,
    accessDialogOpen: false,
    accessScope: 'global',
    accessScopeId: '',
    accessUser: '',
    channelAccessDialogOpen: false,
    channelAccessScope: 'channel',
    channelAccessTarget: '',
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
    SetAccessMode: {
      update: 'root',
      data: ({ root, event }) => ({
        ...root,
        pendingAccessMode:
          root.inFlightAccessMode === null && event.mode === root.serverAccessMode
            ? null
            : event.mode,
        mutationSeq: root.mutationSeq + 1,
        discord: { ...root.discord, accessMode: event.mode },
      }),
    },
    SetChannelAccessMode: {
      update: 'root',
      data: ({ root, event }) => ({
        ...root,
        pendingChannelAccessMode:
          root.inFlightChannelAccessMode === null && event.mode === root.serverChannelAccessMode
            ? null
            : event.mode,
        mutationSeq: root.mutationSeq + 1,
        discord: { ...root.discord, channelAccessMode: event.mode },
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
                  root.inFlightAccessMode !== null ||
                  root.pendingAccessMode !== null ||
                  root.inFlightChannelAccessMode !== null ||
                  root.pendingChannelAccessMode !== null
                ) {
                  return { target: undefined, update: root }
                }
                return {
                  target: undefined,
                  update: {
                    ...root,
                    serverAccessMode: output.discord.accessMode,
                    serverChannelAccessMode: output.discord.channelAccessMode,
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
                          root.accessScope === 'guild' &&
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
              root.pendingAccessMode !== null && root.pendingAccessMode !== root.serverAccessMode,
            target: 'Access.SettingMode',
            update: 'root',
            data: ({ root }) => {
              const mode = root.pendingAccessMode ?? root.serverAccessMode
              return {
                target: { mode },
                update: {
                  ...root,
                  pendingAccessMode: null,
                  inFlightAccessMode: mode,
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
                  accessScope: 'global',
                  accessScopeId: '',
                  accessUser: '',
                },
              }),
            },
            CloseAccessDialog: {
              target: 'Access.Idle',
              update: 'root',
              data: ({ root }) => ({
                target: { error: null },
                update: { ...root, accessDialogOpen: false, accessUser: '' },
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
                    event.scope === 'guild' && root.directory.guilds.length === 1
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
            AddAccessEntry: {
              target: 'Access.Adding',
              update: 'root',
              data: ({ event, root }) => ({
                target: {
                  list: event.list,
                  scope: event.scope,
                  scopeId: event.scopeId,
                  userId: event.userId,
                  refreshDirectory:
                    event.scope === 'channel' &&
                    event.scopeId !== null &&
                    !root.directory.channels.some(channel => channel.id === event.scopeId),
                  tokenGeneration: root.tokenGeneration,
                },
                update: {
                  ...root,
                  inFlightWrites: root.inFlightWrites + 1,
                  mutationSeq: root.mutationSeq + 1,
                },
              }),
            },
            RemoveAccessEntry: {
              target: 'Access.Removing',
              update: 'root',
              data: ({ event, root }) => ({
                target: {
                  list: event.list,
                  scope: event.scope,
                  scopeId: event.scopeId,
                  userId: event.userId,
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
        SettingMode: {
          invoke: {
            src: 'setAccessMode',
            input: ({ state }) => state.mode,
            onDone: {
              target: 'Access.Idle',
              update: 'root',
              data: ({ root, output }) => {
                const nextPending =
                  root.pendingAccessMode === output.accessMode ? null : root.pendingAccessMode
                return {
                  target: { error: null },
                  update: {
                    ...root,
                    serverAccessMode: output.accessMode,
                    pendingAccessMode: nextPending,
                    inFlightAccessMode: null,
                    mutationSeq: root.mutationSeq + 1,
                    discord: {
                      ...root.discord,
                      allowed: output.allowed,
                      accessMode: nextPending ?? output.accessMode,
                      accessEntries: output.accessEntries,
                    },
                    accessUser: '',
                  },
                }
              },
            },
            onFailure: {
              target: 'Access.Idle',
              update: 'root',
              data: ({ root, state, error }) => {
                const nextPending =
                  root.pendingAccessMode === state.mode ||
                  root.pendingAccessMode === root.serverAccessMode
                    ? null
                    : root.pendingAccessMode
                return {
                  target: { error: error.message },
                  update: {
                    ...root,
                    pendingAccessMode: nextPending,
                    inFlightAccessMode: null,
                    mutationSeq: root.mutationSeq + 1,
                    discord: {
                      ...root.discord,
                      accessMode: nextPending ?? root.serverAccessMode,
                    },
                  },
                }
              },
            },
          },
        },
        Adding: {
          invoke: {
            src: 'addAccessEntry',
            input: ({ state }) => ({
              list: state.list,
              scope: state.scope,
              scopeId: state.scopeId,
              userId: state.userId,
              refreshDirectory: state.refreshDirectory,
            }),
            onDone: {
              target: 'Access.Idle',
              update: 'root',
              data: ({ root, state, output }) => {
                const nextPending =
                  root.pendingAccessMode === output.discord.accessMode
                    ? null
                    : root.pendingAccessMode
                return {
                  target: { error: null },
                  update: {
                    ...root,
                    inFlightWrites: Math.max(0, root.inFlightWrites - 1),
                    serverAccessMode: output.discord.accessMode,
                    pendingAccessMode: nextPending,
                    mutationSeq: root.mutationSeq + 1,
                    discord: {
                      ...root.discord,
                      allowed: output.discord.allowed,
                      accessMode: nextPending ?? output.discord.accessMode,
                      accessEntries: output.discord.accessEntries,
                    },
                    directory:
                      state.tokenGeneration === root.tokenGeneration
                        ? mergeDirectoryChannels(root.directory, output.directory)
                        : root.directory,
                    accessDialogOpen: false,
                    accessUser: '',
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
        Removing: {
          invoke: {
            src: 'removeAccessEntry',
            input: ({ state }) => ({
              list: state.list,
              scope: state.scope,
              scopeId: state.scopeId,
              userId: state.userId,
            }),
            onDone: {
              target: 'Access.Idle',
              update: 'root',
              data: ({ root, output }) => {
                const nextPending =
                  root.pendingAccessMode === output.accessMode ? null : root.pendingAccessMode
                return {
                  target: { error: null },
                  update: {
                    ...root,
                    inFlightWrites: Math.max(0, root.inFlightWrites - 1),
                    serverAccessMode: output.accessMode,
                    pendingAccessMode: nextPending,
                    mutationSeq: root.mutationSeq + 1,
                    discord: {
                      ...root.discord,
                      allowed: output.allowed,
                      accessMode: nextPending ?? output.accessMode,
                      accessEntries: output.accessEntries,
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
    ChannelAccess: {
      initial: { target: 'ChannelAccess.Idle', data: { error: null } },
      states: {
        Idle: {
          always: {
            target: 'ChannelAccess.SettingMode',
            guard: ({ root }) => root.pendingChannelAccessMode !== null,
            update: 'root',
            data: ({ root }) => {
              const mode = root.pendingChannelAccessMode ?? root.serverChannelAccessMode
              return {
                target: { mode },
                update: {
                  ...root,
                  inFlightChannelAccessMode: mode,
                  mutationSeq: root.mutationSeq + 1,
                },
              }
            },
          },
          on: {
            OpenChannelAccessDialog: {
              target: 'ChannelAccess.Idle',
              update: 'root',
              data: ({ root }) => ({
                target: { error: null },
                update: {
                  ...root,
                  channelAccessDialogOpen: true,
                  channelAccessScope: 'channel',
                  channelAccessTarget: '',
                },
              }),
            },
            CloseChannelAccessDialog: {
              target: 'ChannelAccess.Idle',
              update: 'root',
              data: ({ root }) => ({
                target: { error: null },
                update: { ...root, channelAccessDialogOpen: false, channelAccessTarget: '' },
              }),
            },
            SelectChannelAccessScope: {
              target: 'ChannelAccess.Idle',
              update: 'root',
              data: ({ event, root, state }) => ({
                target: { ...state, error: null },
                update: {
                  ...root,
                  channelAccessScope: event.scope,
                  channelAccessTarget:
                    event.scope === 'server' && root.directory.guilds.length === 1
                      ? (root.directory.guilds[0]?.id ?? '')
                      : '',
                },
              }),
            },
            SelectChannelAccessTarget: {
              target: 'ChannelAccess.Idle',
              update: 'root',
              data: ({ event, root, state }) => ({
                target: { ...state, error: null },
                update: { ...root, channelAccessTarget: event.targetId },
              }),
            },
            AddChannelAccessEntry: {
              target: 'ChannelAccess.Adding',
              update: 'root',
              data: ({ event, root }) => ({
                target: {
                  list: event.list,
                  scope: event.scope,
                  targetId: event.targetId,
                  refreshDirectory:
                    event.scope === 'channel' &&
                    !root.directory.channels.some(channel => channel.id === event.targetId),
                  tokenGeneration: root.tokenGeneration,
                },
                update: {
                  ...root,
                  inFlightWrites: root.inFlightWrites + 1,
                  mutationSeq: root.mutationSeq + 1,
                },
              }),
            },
            RemoveChannelAccessEntry: {
              target: 'ChannelAccess.Removing',
              update: 'root',
              data: ({ event, root }) => ({
                target: {
                  list: event.list,
                  scope: event.scope,
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
        SettingMode: {
          invoke: {
            src: 'setChannelAccessMode',
            input: ({ state }) => state.mode,
            onDone: {
              target: 'ChannelAccess.Idle',
              update: 'root',
              data: ({ root, output }) => {
                const nextPending =
                  root.pendingChannelAccessMode === output.channelAccessMode
                    ? null
                    : root.pendingChannelAccessMode
                return {
                  target: { error: null },
                  update: {
                    ...root,
                    serverChannelAccessMode: output.channelAccessMode,
                    pendingChannelAccessMode: nextPending,
                    inFlightChannelAccessMode: null,
                    mutationSeq: root.mutationSeq + 1,
                    discord: {
                      ...root.discord,
                      channelAccessMode: nextPending ?? output.channelAccessMode,
                      channelAccessEntries: output.channelAccessEntries,
                    },
                    channelAccessTarget: '',
                  },
                }
              },
            },
            onFailure: {
              target: 'ChannelAccess.Idle',
              update: 'root',
              data: ({ root, state, error }) => {
                const nextPending =
                  root.pendingChannelAccessMode === state.mode ||
                  root.pendingChannelAccessMode === root.serverChannelAccessMode
                    ? null
                    : root.pendingChannelAccessMode
                return {
                  target: { error: error.message },
                  update: {
                    ...root,
                    pendingChannelAccessMode: nextPending,
                    inFlightChannelAccessMode: null,
                    mutationSeq: root.mutationSeq + 1,
                    discord: {
                      ...root.discord,
                      channelAccessMode: nextPending ?? root.serverChannelAccessMode,
                    },
                  },
                }
              },
            },
          },
        },
        Adding: {
          invoke: {
            src: 'addChannelAccessEntry',
            input: ({ state }) => ({
              list: state.list,
              scope: state.scope,
              targetId: state.targetId,
              refreshDirectory: state.refreshDirectory,
            }),
            onDone: {
              target: 'ChannelAccess.Idle',
              update: 'root',
              data: ({ root, state, output }) => {
                const nextPending =
                  root.pendingChannelAccessMode === output.discord.channelAccessMode
                    ? null
                    : root.pendingChannelAccessMode
                return {
                  target: { error: null },
                  update: {
                    ...root,
                    inFlightWrites: Math.max(0, root.inFlightWrites - 1),
                    serverChannelAccessMode: output.discord.channelAccessMode,
                    pendingChannelAccessMode: nextPending,
                    mutationSeq: root.mutationSeq + 1,
                    discord: {
                      ...root.discord,
                      channelAccessMode: nextPending ?? output.discord.channelAccessMode,
                      channelAccessEntries: output.discord.channelAccessEntries,
                    },
                    directory:
                      state.tokenGeneration === root.tokenGeneration
                        ? mergeDirectoryChannels(root.directory, output.directory)
                        : root.directory,
                    channelAccessDialogOpen: false,
                    channelAccessTarget: '',
                  },
                }
              },
            },
            onFailure: {
              target: 'ChannelAccess.Idle',
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
            src: 'removeChannelAccessEntry',
            input: ({ state }) => ({
              list: state.list,
              scope: state.scope,
              targetId: state.targetId,
            }),
            onDone: {
              target: 'ChannelAccess.Idle',
              update: 'root',
              data: ({ root, output }) => {
                const nextPending =
                  root.pendingChannelAccessMode === output.channelAccessMode
                    ? null
                    : root.pendingChannelAccessMode
                return {
                  target: { error: null },
                  update: {
                    ...root,
                    inFlightWrites: Math.max(0, root.inFlightWrites - 1),
                    serverChannelAccessMode: output.channelAccessMode,
                    pendingChannelAccessMode: nextPending,
                    mutationSeq: root.mutationSeq + 1,
                    discord: {
                      ...root.discord,
                      channelAccessMode: nextPending ?? output.channelAccessMode,
                      channelAccessEntries: output.channelAccessEntries,
                    },
                  },
                }
              },
            },
            onFailure: {
              target: 'ChannelAccess.Idle',
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
