import { Schema } from 'effect'

export const Identity = Schema.Struct({
  text: Schema.String,
  isDefault: Schema.Boolean,
})
export type Identity = typeof Identity.Type

export const Reasoning = Schema.Literals([
  'off',
  'low',
  'medium',
  'high',
  'xhigh',
  'max',
])
export type Reasoning = typeof Reasoning.Type

export const Slot = Schema.Struct({
  provider: Schema.String,
  model: Schema.String,
  reasoning: Reasoning,
})
export type Slot = typeof Slot.Type

export const ModelInfo = Schema.Struct({
  id: Schema.String,
  context: Schema.NullOr(Schema.Number),
  limit: Schema.NullOr(Schema.Number),
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
  modelCount: Schema.Number,
})
export type Provider = typeof Provider.Type

export const Home = Schema.Union([
  Schema.Struct({ kind: Schema.Literal('none') }),
  Schema.Struct({ kind: Schema.Literal('channel'), id: Schema.String }),
  Schema.Struct({ kind: Schema.Literal('dm'), user: Schema.String }),
])
export type Home = typeof Home.Type

export const Discord = Schema.Struct({
  allowed: Schema.Array(Schema.String),
  home: Home,
})
export type Discord = typeof Discord.Type

export const DiscordChannel = Schema.Struct({
  id: Schema.String,
  name: Schema.String,
  server: Schema.String,
})
export type DiscordChannel = typeof DiscordChannel.Type

export const DiscordPerson = Schema.Struct({
  id: Schema.String,
  name: Schema.String,
})
export type DiscordPerson = typeof DiscordPerson.Type

export const DiscordDirectory = Schema.Struct({
  channels: Schema.Array(DiscordChannel),
  people: Schema.Array(DiscordPerson),
})
export type DiscordDirectory = typeof DiscordDirectory.Type

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
      limit: Schema.NullOr(Schema.Number),
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
}

export type RequestType = keyof typeof requests

export type RequestPayload<K extends RequestType> =
  (typeof requests)[K]['payload']['Type']

export type ReplyPayload<K extends RequestType> =
  (typeof requests)[K]['reply']['Type']

export const isRequestType = (type: string): type is RequestType =>
  Object.hasOwn(requests, type)

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
