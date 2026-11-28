import { Schema } from 'effect'

export const Instructions = Schema.Struct({
  text: Schema.String,
  isDefault: Schema.Boolean,
})
export type Instructions = typeof Instructions.Type

export const GeneralSettings = Schema.Struct({
  autoRespond: Schema.Boolean,
  hasUtilityModel: Schema.Boolean,
  webHome: Schema.Boolean,
})
export type GeneralSettings = typeof GeneralSettings.Type

export const ChatMessage = Schema.Struct({
  id: Schema.Finite,
  bot: Schema.Boolean,
  text: Schema.String,
  createdAt: Schema.Finite,
})
export type ChatMessage = typeof ChatMessage.Type

/** A page of a bot's web chat, oldest first, with the bot's live status. */
export const ChatPage = Schema.Struct({
  messages: Schema.Array(ChatMessage),
  hasMore: Schema.Boolean,
  working: Schema.Boolean,
  typingUntil: Schema.NullOr(Schema.Finite),
})
export type ChatPage = typeof ChatPage.Type

export const UsageKind = Schema.Literals(['chat', 'task', 'utility', 'compaction'])
export type UsageKind = typeof UsageKind.Type

export const UsageTotals = Schema.Struct({
  calls: Schema.Finite,
  inputTokens: Schema.Finite,
  cachedTokens: Schema.Finite,
  outputTokens: Schema.Finite,
})
export type UsageTotals = typeof UsageTotals.Type

export const UsageKindBucket = Schema.Struct({
  kind: UsageKind,
  calls: Schema.Finite,
  inputTokens: Schema.Finite,
  cachedTokens: Schema.Finite,
  outputTokens: Schema.Finite,
})
export type UsageKindBucket = typeof UsageKindBucket.Type

export const UsageModelBucket = Schema.Struct({
  provider: Schema.String,
  model: Schema.String,
  calls: Schema.Finite,
  inputTokens: Schema.Finite,
  cachedTokens: Schema.Finite,
  outputTokens: Schema.Finite,
})
export type UsageModelBucket = typeof UsageModelBucket.Type

export const UsageDayBucket = Schema.Struct({
  day: Schema.String,
  calls: Schema.Finite,
  inputTokens: Schema.Finite,
  cachedTokens: Schema.Finite,
  outputTokens: Schema.Finite,
})
export type UsageDayBucket = typeof UsageDayBucket.Type

export const UsageSummary = Schema.Struct({
  days: Schema.Finite,
  totals: UsageTotals,
  byKind: Schema.Array(UsageKindBucket),
  byModel: Schema.Array(UsageModelBucket),
  daily: Schema.Array(UsageDayBucket),
})
export type UsageSummary = typeof UsageSummary.Type

export const Webhook = Schema.Struct({
  id: Schema.String,
  name: Schema.String,
  token: Schema.String,
  enabled: Schema.Boolean,
  createdAt: Schema.Finite,
  lastTriggeredAt: Schema.NullOr(Schema.Finite),
})
export type Webhook = typeof Webhook.Type

export const WebhooksPayload = Schema.Struct({
  publicUrl: Schema.NullOr(Schema.String),
  webhooks: Schema.Array(Webhook),
})
export type WebhooksPayload = typeof WebhooksPayload.Type

export const Secret = Schema.Struct({
  name: Schema.String,
  createdAt: Schema.Finite,
  updatedAt: Schema.Finite,
})
export type Secret = typeof Secret.Type

export const SecretsPayload = Schema.Struct({
  publicUrl: Schema.NullOr(Schema.String),
  secrets: Schema.Array(Secret),
})
export type SecretsPayload = typeof SecretsPayload.Type

export const SecretRequestLink = Schema.Struct({
  url: Schema.String,
  expiresAt: Schema.Finite,
})
export type SecretRequestLink = typeof SecretRequestLink.Type

export const Reasoning = Schema.Literals(['off', 'low', 'medium', 'high', 'xhigh', 'max'])
export type Reasoning = typeof Reasoning.Type

export const Slot = Schema.Struct({
  provider: Schema.String,
  model: Schema.String,
  reasoning: Reasoning,
})
export type Slot = typeof Slot.Type

export const DecisionSlot = Schema.Struct({
  provider: Schema.String,
  model: Schema.String,
})
export type DecisionSlot = typeof DecisionSlot.Type

export const ModelInfo = Schema.Struct({
  id: Schema.String,
  context: Schema.NullOr(Schema.Finite),
  limit: Schema.NullOr(Schema.Finite),
  images: Schema.Boolean,
  enabled: Schema.Boolean,
})
export type ModelInfo = typeof ModelInfo.Type

export const ProviderApi = Schema.Literals(['responses', 'completions', 'opencode-go', 'decision'])
export type ProviderApi = typeof ProviderApi.Type

export const ProviderModels = Schema.Struct({
  id: Schema.String,
  api: ProviderApi,
  models: Schema.Array(ModelInfo),
})
export type ProviderModels = typeof ProviderModels.Type

export const Models = Schema.Struct({
  main: Slot,
  compaction: Schema.NullOr(Slot),
  utility: Schema.NullOr(Slot),
  decision: Schema.NullOr(DecisionSlot),
  providers: Schema.Array(ProviderModels),
})
export type Models = typeof Models.Type

export const Provider = Schema.Struct({
  id: Schema.String,
  api: ProviderApi,
  baseUrl: Schema.String,
  apiKey: Schema.Literals(['set', 'missing']),
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

export const BotSummary = Schema.Struct({
  id: Schema.String,
  name: Schema.String,
  enabled: Schema.Boolean,
  isDefault: Schema.Boolean,
  createdAt: Schema.Finite,
  /** Monotonic generation: never reused for a recreated slug, so it identifies one bot instance. */
  generation: Schema.Finite,
  /** Custom avatar upload time, versions the `/api/bots/{id}/avatar?v=` URL; null means generated. */
  avatarAt: Schema.NullOr(Schema.Finite),
  mainModel: Schema.NullOr(Schema.String),
  connection: DiscordConnection,
  /** 'local' or the name of the remote execution environment this bot runs on. */
  environment: Schema.String,
  environmentOnline: Schema.Boolean,
})
export type BotSummary = typeof BotSummary.Type

/** A node that connected and is waiting to be accepted as an environment. */
export const PendingNode = Schema.Struct({
  nodeId: Schema.String,
  hostname: Schema.String,
  version: Schema.String,
  /** Unix epoch seconds. */
  connectedAt: Schema.Finite,
})
export type PendingNode = typeof PendingNode.Type

export const EnvironmentInfo = Schema.Struct({
  name: Schema.String,
  nodeId: Schema.String,
  version: Schema.NullOr(Schema.String),
  /** Unix epoch seconds. */
  lastSeen: Schema.Finite,
  online: Schema.Boolean,
})
export type EnvironmentInfo = typeof EnvironmentInfo.Type

export const Environments = Schema.Struct({
  pending: Schema.Array(PendingNode),
  environments: Schema.Array(EnvironmentInfo),
})
export type Environments = typeof Environments.Type

export const BotMcpServer = Schema.Struct({
  id: Schema.String,
  enabledGlobal: Schema.Boolean,
  enabledForBot: Schema.Boolean,
})
export type BotMcpServer = typeof BotMcpServer.Type

export const BotMcpState = Schema.Struct({
  servers: Schema.Array(BotMcpServer),
})
export type BotMcpState = typeof BotMcpState.Type

export const BotSkillMode = Schema.Literals(['both', 'chat', 'task', 'off'])
export type BotSkillMode = typeof BotSkillMode.Type

export const BotSkill = Schema.Struct({
  name: Schema.String,
  description: Schema.String,
  source: Schema.Literals(['managed', 'detected']),
  mode: BotSkillMode,
})
export type BotSkill = typeof BotSkill.Type

/** A skill's full description, `SKILL.md` body without frontmatter, and other file paths. */
export const BotSkillDetail = Schema.Struct({
  name: Schema.String,
  description: Schema.String,
  source: Schema.Literals(['managed', 'detected']),
  content: Schema.String,
  files: Schema.Array(Schema.String),
})
export type BotSkillDetail = typeof BotSkillDetail.Type

export const BotSkillsState = Schema.Struct({
  skills: Schema.Array(BotSkill),
})
export type BotSkillsState = typeof BotSkillsState.Type

export const GithubSkillPreview = Schema.Struct({
  folder: Schema.String,
  name: Schema.String,
  description: Schema.String,
  files: Schema.Finite,
  bytes: Schema.Finite,
  conflict: Schema.NullOr(Schema.Literals(['managed', 'detected'])),
})
export type GithubSkillPreview = typeof GithubSkillPreview.Type

export const GithubSkillsPreview = Schema.Struct({
  repo: Schema.String,
  commit: Schema.String,
  path: Schema.String,
  skills: Schema.Array(GithubSkillPreview),
})
export type GithubSkillsPreview = typeof GithubSkillsPreview.Type

export const DiscordInteractionScope = Schema.Literals(['channel', 'user', 'server'])
export type DiscordInteractionScope = typeof DiscordInteractionScope.Type

export const DiscordInteractionMode = Schema.Literals(['mention-only', 'always', 'auto'])
export type DiscordInteractionMode = typeof DiscordInteractionMode.Type

export const DiscordInteractionOverride = Schema.Struct({
  scope: DiscordInteractionScope,
  guildId: Schema.NullOr(Schema.String),
  targetId: Schema.String,
  mode: DiscordInteractionMode,
})
export type DiscordInteractionOverride = typeof DiscordInteractionOverride.Type

export const DiscordReplyScope = Schema.Literals(['channel', 'server'])
export type DiscordReplyScope = typeof DiscordReplyScope.Type

export const DiscordReplyMode = Schema.Literals(['auto', 'thread', 'direct'])
export type DiscordReplyMode = typeof DiscordReplyMode.Type

export const DiscordReplyOverride = Schema.Struct({
  scope: DiscordReplyScope,
  targetId: Schema.String,
  mode: DiscordReplyMode,
})
export type DiscordReplyOverride = typeof DiscordReplyOverride.Type

export const DiscordAccessEffect = Schema.Literals(['allow', 'block'])
export type DiscordAccessEffect = typeof DiscordAccessEffect.Type

export const DiscordAccessScope = Schema.Literals(['global', 'server', 'channel'])
export type DiscordAccessScope = typeof DiscordAccessScope.Type

export const DiscordAccessRule = Schema.Struct({
  userId: Schema.NullOr(Schema.String),
  scope: DiscordAccessScope,
  scopeId: Schema.NullOr(Schema.String),
  effect: DiscordAccessEffect,
})
export type DiscordAccessRule = typeof DiscordAccessRule.Type

export const Discord = Schema.Struct({
  allowed: Schema.Array(Schema.String),
  defaultAccess: DiscordAccessEffect,
  accessRules: Schema.Array(DiscordAccessRule),
  home: Home,
  token: Schema.Literals(['set', 'missing']),
  connection: DiscordConnection,
  interactionModes: Schema.Array(DiscordInteractionOverride),
  defaultInteractionMode: DiscordInteractionMode,
  replyModes: Schema.Array(DiscordReplyOverride),
  defaultReplyMode: DiscordReplyMode,
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
  guildId: Schema.String,
  kind: Schema.Literals(['channel', 'thread']),
})
export type DiscordChannel = typeof DiscordChannel.Type

export const DiscordPerson = Schema.Struct({
  id: Schema.String,
  name: Schema.String,
  bot: Schema.Boolean,
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
const BotScopedPayload = Schema.Struct({
  botId: Schema.optional(Schema.String),
  botGeneration: Schema.optional(Schema.Finite),
})
const botIdField = {
  botId: Schema.optional(Schema.String),
  botGeneration: Schema.optional(Schema.Finite),
} as const

export const Weekday = Schema.Literals(['mon', 'tue', 'wed', 'thu', 'fri', 'sat', 'sun'])
export type Weekday = typeof Weekday.Type

export const ScheduleKind = Schema.Literals(['at', 'cron'])
export type ScheduleKind = typeof ScheduleKind.Type

export const ScheduleItem = Schema.Struct({
  id: Schema.String,
  text: Schema.String,
  kind: ScheduleKind,
  when: Schema.String,
  timezone: Schema.String,
  nextDue: Schema.Finite,
  target: Schema.NullOr(Schema.String),
})
export type ScheduleItem = typeof ScheduleItem.Type

export const CheckinConfig = Schema.Struct({
  every: Schema.Finite,
  days: Schema.Array(Weekday),
  startHour: Schema.Finite,
  endHour: Schema.Finite,
})
export type CheckinConfig = typeof CheckinConfig.Type

export const WatchItem = Schema.Struct({
  id: Schema.String,
  text: Schema.String,
  until: Schema.NullOr(Schema.Finite),
  target: Schema.NullOr(Schema.String),
})
export type WatchItem = typeof WatchItem.Type

export const ScheduleOccurrence = Schema.Struct({
  scheduleId: Schema.String,
  at: Schema.Finite,
})
export type ScheduleOccurrence = typeof ScheduleOccurrence.Type

export const ScheduleOccurrenceCursor = Schema.Struct({
  at: Schema.Finite,
  scheduleId: Schema.String,
})
export type ScheduleOccurrenceCursor = typeof ScheduleOccurrenceCursor.Type

export const ScheduleOccurrencesPage = Schema.Struct({
  occurrences: Schema.Array(ScheduleOccurrence),
  nextCursor: Schema.NullOr(ScheduleOccurrenceCursor),
})
export type ScheduleOccurrencesPage = typeof ScheduleOccurrencesPage.Type

export const ScheduleState = Schema.Struct({
  timezone: Schema.String,
  schedules: Schema.Array(ScheduleItem),
  checkin: Schema.NullOr(CheckinConfig),
  nextCheckin: Schema.NullOr(Schema.Finite),
  watches: Schema.Array(WatchItem),
})
export type ScheduleState = typeof ScheduleState.Type

export const requests = {
  'bots.list': { payload: NoPayload, reply: Schema.Array(BotSummary) },
  'bots.create': {
    payload: Schema.Struct({ id: Schema.String, name: Schema.String }),
    reply: Schema.Array(BotSummary),
  },
  'bots.update': {
    payload: Schema.Struct({
      id: Schema.String,
      name: Schema.optional(Schema.NullOr(Schema.String)),
      enabled: Schema.optional(Schema.NullOr(Schema.Boolean)),
      botGeneration: Schema.optional(Schema.Finite),
    }),
    reply: Schema.Array(BotSummary),
  },
  'bots.remove': {
    payload: Schema.Struct({ id: Schema.String, botGeneration: Schema.optional(Schema.Finite) }),
    reply: Schema.Array(BotSummary),
  },
  'bots.setEnvironment': {
    payload: Schema.Struct({
      botId: Schema.String,
      environment: Schema.String,
      botGeneration: Schema.optional(Schema.Finite),
    }),
    reply: Schema.Array(BotSummary),
  },
  'bot.mcp.get': { payload: BotScopedPayload, reply: BotMcpState },
  'bot.skills.list': { payload: BotScopedPayload, reply: BotSkillsState },
  'bot.skills.setMode': {
    payload: Schema.Struct({ ...botIdField, name: Schema.String, mode: BotSkillMode }),
    reply: BotSkillsState,
  },
  'bot.skills.remove': {
    payload: Schema.Struct({ ...botIdField, name: Schema.String }),
    reply: BotSkillsState,
  },
  'bot.skills.githubPreview': {
    payload: Schema.Struct({ ...botIdField, url: Schema.String, path: Schema.String }),
    reply: GithubSkillsPreview,
  },
  'bot.skills.githubImport': {
    payload: Schema.Struct({
      ...botIdField,
      repo: Schema.String,
      commit: Schema.String,
      folders: Schema.Array(Schema.String),
    }),
    reply: BotSkillsState,
  },
  'bot.skills.importFromBot': {
    payload: Schema.Struct({
      ...botIdField,
      fromBotId: Schema.String,
      fromBotGeneration: Schema.Finite,
      names: Schema.Array(Schema.String),
    }),
    reply: BotSkillsState,
  },
  'bot.skills.read': {
    payload: Schema.Struct({ ...botIdField, name: Schema.String }),
    reply: BotSkillDetail,
  },
  'bot.skills.adopt': {
    payload: Schema.Struct({ ...botIdField, name: Schema.String }),
    reply: BotSkillsState,
  },
  'bot.mcp.setEnabled': {
    payload: Schema.Struct({ ...botIdField, id: Schema.String, enabled: Schema.Boolean }),
    reply: BotMcpState,
  },
  'environments.list': { payload: NoPayload, reply: Environments },
  'environments.accept': {
    payload: Schema.Struct({
      nodeId: Schema.String,
      name: Schema.optional(Schema.String),
    }),
    reply: Environments,
  },
  'environments.remove': {
    payload: Schema.Struct({ name: Schema.String }),
    reply: Environments,
  },
  'environments.rename': {
    payload: Schema.Struct({ name: Schema.String, to: Schema.String }),
    reply: Environments,
  },
  'general.get': { payload: BotScopedPayload, reply: GeneralSettings },
  'general.setAutoRespond': {
    payload: Schema.Struct({ ...botIdField, enabled: Schema.Boolean }),
    reply: GeneralSettings,
  },
  'general.setWebHome': {
    payload: Schema.Struct({ ...botIdField, enabled: Schema.Boolean }),
    reply: GeneralSettings,
  },
  'chat.history': {
    payload: Schema.Struct({ ...botIdField, before: Schema.NullOr(Schema.Finite) }),
    reply: ChatPage,
  },
  'chat.send': {
    payload: Schema.Struct({ ...botIdField, text: Schema.String }),
    reply: ChatPage,
  },
  'instructions.get': { payload: BotScopedPayload, reply: Instructions },
  'instructions.set': {
    payload: Schema.Struct({ ...botIdField, text: Schema.String }),
    reply: Instructions,
  },
  'instructions.reset': { payload: BotScopedPayload, reply: Instructions },
  'models.get': { payload: BotScopedPayload, reply: Models },
  'models.setMain': {
    payload: Schema.Struct({
      ...botIdField,
      provider: Schema.String,
      model: Schema.String,
      reasoning: Reasoning,
    }),
    reply: Models,
  },
  'models.setCompaction': {
    payload: Schema.Struct({ ...botIdField, slot: Schema.NullOr(Slot) }),
    reply: Models,
  },
  'models.setUtility': {
    payload: Schema.Struct({ ...botIdField, slot: Schema.NullOr(Slot) }),
    reply: Models,
  },
  'models.setDecision': {
    payload: Schema.Struct({ ...botIdField, slot: Schema.NullOr(DecisionSlot) }),
    reply: Models,
  },
  'models.setContextLimit': {
    payload: Schema.Struct({
      ...botIdField,
      provider: Schema.String,
      model: Schema.String,
      limit: Schema.NullOr(Schema.Finite),
    }),
    reply: Models,
  },
  'models.setImages': {
    payload: Schema.Struct({
      ...botIdField,
      provider: Schema.String,
      model: Schema.String,
      images: Schema.Boolean,
    }),
    reply: Models,
  },
  'models.setEnabled': {
    payload: Schema.Struct({
      ...botIdField,
      provider: Schema.String,
      model: Schema.String,
      enabled: Schema.Boolean,
    }),
    reply: Models,
  },
  'providers.list': { payload: NoPayload, reply: Schema.Array(Provider) },
  'providers.add': {
    payload: Schema.Struct({
      id: Schema.String,
      api: ProviderApi,
      baseUrl: Schema.String,
      apiKey: Schema.NullOr(Schema.String),
      models: Schema.optional(Schema.Array(Schema.String)),
    }),
    reply: Schema.Array(Provider),
  },
  'providers.setKey': {
    payload: Schema.Struct({
      id: Schema.String,
      apiKey: Schema.NullOr(Schema.String),
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
  'discord.get': { payload: BotScopedPayload, reply: Discord },
  'discord.directory': { payload: BotScopedPayload, reply: DiscordDirectory },
  'discord.allow': {
    payload: Schema.Struct({ ...botIdField, user: Schema.String }),
    reply: Discord,
  },
  'discord.disallow': {
    payload: Schema.Struct({ ...botIdField, user: Schema.String }),
    reply: Discord,
  },
  'discord.setHome': {
    payload: Schema.Struct({ ...botIdField, home: Home }),
    reply: Discord,
  },
  'discord.setToken': {
    payload: Schema.Struct({ ...botIdField, token: Schema.String }),
    reply: Discord,
  },
  'discord.clearToken': { payload: BotScopedPayload, reply: Discord },
  'discord.setInteractionMode': {
    payload: Schema.Struct({
      ...botIdField,
      scope: DiscordInteractionScope,
      guildId: Schema.NullOr(Schema.String),
      targetId: Schema.String,
      mode: DiscordInteractionMode,
    }),
    reply: Discord,
  },
  'discord.clearInteractionMode': {
    payload: Schema.Struct({
      ...botIdField,
      scope: DiscordInteractionScope,
      guildId: Schema.NullOr(Schema.String),
      targetId: Schema.String,
    }),
    reply: Discord,
  },
  'discord.setDefaultInteractionMode': {
    payload: Schema.Struct({
      ...botIdField,
      mode: DiscordInteractionMode,
    }),
    reply: Discord,
  },
  'discord.setReplyMode': {
    payload: Schema.Struct({
      ...botIdField,
      scope: DiscordReplyScope,
      targetId: Schema.String,
      mode: DiscordReplyMode,
    }),
    reply: Discord,
  },
  'discord.clearReplyMode': {
    payload: Schema.Struct({
      ...botIdField,
      scope: DiscordReplyScope,
      targetId: Schema.String,
    }),
    reply: Discord,
  },
  'discord.setDefaultReplyMode': {
    payload: Schema.Struct({
      ...botIdField,
      mode: DiscordReplyMode,
    }),
    reply: Discord,
  },
  'discord.setDefaultAccess': {
    payload: Schema.Struct({
      ...botIdField,
      effect: DiscordAccessEffect,
    }),
    reply: Discord,
  },
  'discord.setAccessRule': {
    payload: Schema.Struct({
      ...botIdField,
      userId: Schema.NullOr(Schema.String),
      scope: DiscordAccessScope,
      scopeId: Schema.NullOr(Schema.String),
      effect: DiscordAccessEffect,
    }),
    reply: Discord,
  },
  'discord.removeAccessRule': {
    payload: Schema.Struct({
      ...botIdField,
      userId: Schema.NullOr(Schema.String),
      scope: DiscordAccessScope,
      scopeId: Schema.NullOr(Schema.String),
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
  'schedule.get': { payload: BotScopedPayload, reply: ScheduleState },
  'schedule.occurrences': {
    payload: Schema.Struct({
      ...botIdField,
      start: Schema.Finite,
      end: Schema.Finite,
      cursor: Schema.optional(Schema.NullOr(ScheduleOccurrenceCursor)),
    }),
    reply: ScheduleOccurrencesPage,
  },
  'schedule.setTimezone': {
    payload: Schema.Struct({ ...botIdField, timezone: Schema.String }),
    reply: ScheduleState,
  },
  'schedule.add': {
    payload: Schema.Struct({
      ...botIdField,
      text: Schema.String,
      kind: ScheduleKind,
      when: Schema.String,
      timezone: Schema.NullOr(Schema.String),
    }),
    reply: ScheduleState,
  },
  'schedule.remove': {
    payload: Schema.Struct({ ...botIdField, id: Schema.String }),
    reply: ScheduleState,
  },
  'checkin.set': {
    payload: Schema.Struct({
      ...botIdField,
      every: Schema.Finite,
      days: Schema.Array(Weekday),
      startHour: Schema.Finite,
      endHour: Schema.Finite,
    }),
    reply: ScheduleState,
  },
  'checkin.clear': { payload: BotScopedPayload, reply: ScheduleState },
  'watch.add': {
    payload: Schema.Struct({
      ...botIdField,
      text: Schema.String,
      until: Schema.NullOr(Schema.String),
    }),
    reply: ScheduleState,
  },
  'watch.remove': {
    payload: Schema.Struct({ ...botIdField, id: Schema.String }),
    reply: ScheduleState,
  },
  'usage.get': {
    payload: Schema.Struct({ ...botIdField, days: Schema.Finite }),
    reply: UsageSummary,
  },
  'webhooks.list': {
    payload: BotScopedPayload,
    reply: WebhooksPayload,
  },
  'webhooks.create': {
    payload: Schema.Struct({ ...botIdField, name: Schema.String }),
    reply: WebhooksPayload,
  },
  'webhooks.setEnabled': {
    payload: Schema.Struct({ ...botIdField, id: Schema.String, enabled: Schema.Boolean }),
    reply: WebhooksPayload,
  },
  'webhooks.remove': {
    payload: Schema.Struct({ ...botIdField, id: Schema.String }),
    reply: WebhooksPayload,
  },
  'secrets.list': { payload: BotScopedPayload, reply: SecretsPayload },
  'secrets.set': {
    payload: Schema.Struct({ ...botIdField, name: Schema.String, value: Schema.String }),
    reply: SecretsPayload,
  },
  'secrets.remove': {
    payload: Schema.Struct({ ...botIdField, name: Schema.String }),
    reply: SecretsPayload,
  },
  'secrets.importFromBot': {
    payload: Schema.Struct({
      ...botIdField,
      fromBotId: Schema.String,
      fromBotGeneration: Schema.Finite,
      names: Schema.Array(Schema.String),
    }),
    reply: SecretsPayload,
  },
  'secrets.request': {
    payload: Schema.Struct({
      ...botIdField,
      name: Schema.String,
      label: Schema.NullOr(Schema.String),
    }),
    reply: SecretRequestLink,
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

/** Topics of bot data the server pushes `changed` frames for. */
export const ChangeTopic = Schema.Literals(['schedule', 'usage', 'webhooks', 'secrets', 'chat'])
export type ChangeTopic = typeof ChangeTopic.Type

/** Server push on `/ws?changes=1`: a bot's data under `topic` changed, so refetch it. */
export const ChangeFrame = Schema.Struct({
  type: Schema.Literal('changed'),
  botId: Schema.String,
  topic: ChangeTopic,
})
export type ChangeFrame = typeof ChangeFrame.Type
