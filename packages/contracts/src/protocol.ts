import { Schema } from 'effect'

export const Identity = Schema.Struct({
  text: Schema.String,
  isDefault: Schema.Boolean,
})
export type Identity = typeof Identity.Type

export const Reasoning = Schema.Literals(['off', 'low', 'medium', 'high', 'xhigh', 'max'])
export type Reasoning = typeof Reasoning.Type

export const Slot = Schema.Struct({
  provider: Schema.String,
  model: Schema.String,
  reasoning: Reasoning,
})
export type Slot = typeof Slot.Type

export const ModelInfo = Schema.Struct({
  id: Schema.String,
  context: Schema.NullOr(Schema.Finite),
  limit: Schema.NullOr(Schema.Finite),
  images: Schema.Boolean,
})
export type ModelInfo = typeof ModelInfo.Type

export const ProviderModels = Schema.Struct({
  id: Schema.String,
  models: Schema.Array(ModelInfo),
})
export type ProviderModels = typeof ProviderModels.Type

export const Models = Schema.Struct({
  main: Slot,
  compaction: Schema.NullOr(Slot),
  utility: Schema.NullOr(Slot),
  providers: Schema.Array(ProviderModels),
})
export type Models = typeof Models.Type

export const ProviderApi = Schema.Literals(['responses', 'completions'])
export type ProviderApi = typeof ProviderApi.Type

export const Provider = Schema.Struct({
  id: Schema.String,
  api: ProviderApi,
  baseUrl: Schema.String,
  keyEnv: Schema.String,
  modelCount: Schema.Finite,
})
export type Provider = typeof Provider.Type

export const Home = Schema.Union([
  Schema.Struct({ kind: Schema.Literal('none') }),
  Schema.Struct({ kind: Schema.Literal('channel'), id: Schema.String }),
  Schema.Struct({ kind: Schema.Literal('dm'), user: Schema.String }),
])
export type Home = typeof Home.Type

export const DiscordConnection = Schema.Union([
  Schema.Struct({ state: Schema.Literal('off') }),
  Schema.Struct({ state: Schema.Literal('connecting') }),
  Schema.Struct({ state: Schema.Literal('online'), bot: Schema.String }),
  Schema.Struct({ state: Schema.Literal('error'), message: Schema.String }),
])
export type DiscordConnection = typeof DiscordConnection.Type

export const DiscordInteractionScope = Schema.Literals(['channel', 'user'])
export type DiscordInteractionScope = typeof DiscordInteractionScope.Type

export const DiscordInteractionMode = Schema.Literals(['mention-only', 'always'])
export type DiscordInteractionMode = typeof DiscordInteractionMode.Type

export const DiscordInteractionOverride = Schema.Struct({
  scope: DiscordInteractionScope,
  guildId: Schema.NullOr(Schema.String),
  targetId: Schema.String,
  mode: DiscordInteractionMode,
})
export type DiscordInteractionOverride = typeof DiscordInteractionOverride.Type

export const Discord = Schema.Struct({
  allowed: Schema.Array(Schema.String),
  home: Home,
  token: Schema.Literals(['set', 'missing']),
  connection: DiscordConnection,
  interactionModes: Schema.Array(DiscordInteractionOverride),
})
export type Discord = typeof Discord.Type

export const DiscordGuild = Schema.Struct({
  id: Schema.String,
  name: Schema.String,
})
export type DiscordGuild = typeof DiscordGuild.Type

export const DiscordChannel = Schema.Struct({
  id: Schema.String,
  name: Schema.String,
  server: Schema.String,
})
export type DiscordChannel = typeof DiscordChannel.Type

export const DiscordPerson = Schema.Struct({
  id: Schema.String,
  name: Schema.String,
  guilds: Schema.Array(Schema.String),
})
export type DiscordPerson = typeof DiscordPerson.Type

export const DiscordDirectory = Schema.Struct({
  guilds: Schema.Array(DiscordGuild),
  channels: Schema.Array(DiscordChannel),
  people: Schema.Array(DiscordPerson),
})
export type DiscordDirectory = typeof DiscordDirectory.Type

export const McpServer = Schema.Struct({
  id: Schema.String,
  transport: Schema.Literals(['stdio', 'http']),
  command: Schema.optional(Schema.String),
  args: Schema.optional(Schema.Array(Schema.String)),
  url: Schema.optional(Schema.String),
  enabled: Schema.Boolean,
  auth: Schema.Literals(['none', 'headers', 'oauth']),
  status: Schema.Literals([
    'connected',
    'connecting',
    'idle',
    'disconnected',
    'needs-login',
    'error',
  ]),
  error: Schema.optional(Schema.String),
  toolCount: Schema.optional(Schema.Finite),
})
export type McpServer = typeof McpServer.Type

const StringMap = Schema.Record(Schema.String, Schema.String)

export const McpAddPayload = Schema.Union([
  Schema.Struct({
    id: Schema.String,
    transport: Schema.Literal('stdio'),
    command: Schema.String,
    args: Schema.optional(Schema.Array(Schema.String)),
    env: Schema.optional(StringMap),
  }),
  Schema.Struct({
    id: Schema.String,
    transport: Schema.Literal('http'),
    url: Schema.String,
    headers: Schema.optional(StringMap),
  }),
])

const NoPayload = Schema.Struct({})

export const requests = {
  'identity.get': { payload: NoPayload, reply: Identity },
  'identity.set': {
    payload: Schema.Struct({ text: Schema.String }),
    reply: Identity,
  },
  'identity.reset': { payload: NoPayload, reply: Identity },
  'models.get': { payload: NoPayload, reply: Models },
  'models.setMain': {
    payload: Schema.Struct({
      provider: Schema.String,
      model: Schema.String,
      reasoning: Reasoning,
    }),
    reply: Models,
  },
  'models.setCompaction': {
    payload: Schema.Struct({ slot: Schema.NullOr(Slot) }),
    reply: Models,
  },
  'models.setUtility': {
    payload: Schema.Struct({ slot: Schema.NullOr(Slot) }),
    reply: Models,
  },
  'models.setContextLimit': {
    payload: Schema.Struct({
      provider: Schema.String,
      model: Schema.String,
      limit: Schema.NullOr(Schema.Finite),
    }),
    reply: Models,
  },
  'models.setImages': {
    payload: Schema.Struct({
      provider: Schema.String,
      model: Schema.String,
      images: Schema.Boolean,
    }),
    reply: Models,
  },
  'providers.list': { payload: NoPayload, reply: Schema.Array(Provider) },
  'providers.add': {
    payload: Schema.Struct({
      id: Schema.String,
      api: ProviderApi,
      baseUrl: Schema.String,
      keyEnv: Schema.String,
    }),
    reply: Schema.Array(Provider),
  },
  'providers.remove': {
    payload: Schema.Struct({ id: Schema.String }),
    reply: Schema.Array(Provider),
  },
  'providers.refresh': {
    payload: Schema.Struct({ id: Schema.String }),
    reply: Schema.Array(Provider),
  },
  'discord.get': { payload: NoPayload, reply: Discord },
  'discord.directory': { payload: NoPayload, reply: DiscordDirectory },
  'discord.allow': {
    payload: Schema.Struct({ user: Schema.String }),
    reply: Discord,
  },
  'discord.disallow': {
    payload: Schema.Struct({ user: Schema.String }),
    reply: Discord,
  },
  'discord.setHome': {
    payload: Schema.Struct({ home: Home }),
    reply: Discord,
  },
  'discord.setToken': {
    payload: Schema.Struct({ token: Schema.String }),
    reply: Discord,
  },
  'discord.clearToken': { payload: NoPayload, reply: Discord },
  'discord.setInteractionMode': {
    payload: Schema.Struct({
      scope: DiscordInteractionScope,
      guildId: Schema.NullOr(Schema.String),
      targetId: Schema.String,
      mode: DiscordInteractionMode,
    }),
    reply: Discord,
  },
  'discord.clearInteractionMode': {
    payload: Schema.Struct({
      scope: DiscordInteractionScope,
      guildId: Schema.NullOr(Schema.String),
      targetId: Schema.String,
    }),
    reply: Discord,
  },
  'mcp.list': { payload: NoPayload, reply: Schema.Array(McpServer) },
  'mcp.add': { payload: McpAddPayload, reply: Schema.Array(McpServer) },
  'mcp.remove': {
    payload: Schema.Struct({ id: Schema.String }),
    reply: Schema.Array(McpServer),
  },
  'mcp.setEnabled': {
    payload: Schema.Struct({ id: Schema.String, enabled: Schema.Boolean }),
    reply: Schema.Array(McpServer),
  },
  'mcp.login': {
    payload: Schema.Struct({ id: Schema.String }),
    reply: Schema.Struct({ url: Schema.String }),
  },
}

export type RequestType = keyof typeof requests

export type RequestPayload<K extends RequestType> = (typeof requests)[K]['payload']['Type']

export type ReplyPayload<K extends RequestType> = (typeof requests)[K]['reply']['Type']

export const isRequestType = (type: string): type is RequestType => Object.hasOwn(requests, type)

export const RequestFrame = Schema.Struct({
  id: Schema.String,
  type: Schema.String,
  payload: Schema.Unknown,
})
export type RequestFrame = typeof RequestFrame.Type

export const ReplyFrame = Schema.Union([
  Schema.Struct({
    id: Schema.String,
    ok: Schema.Literal(true),
    payload: Schema.Unknown,
  }),
  Schema.Struct({
    id: Schema.String,
    ok: Schema.Literal(false),
    error: Schema.String,
  }),
])
export type ReplyFrame = typeof ReplyFrame.Type
