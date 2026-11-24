import { Array, Duration, Effect, Layer, Random, Ref, Schema, Stream, Struct } from 'effect'

import { Transport } from './api.ts'
import {
  type BotMcpState,
  type BotSummary,
  type CheckinConfig,
  type DecisionSlot,
  Discord,
  type DiscordAccessEffect,
  type DiscordAccessRule,
  type DiscordInteractionMode,
  type DiscordInteractionOverride,
  type DiscordReplyMode,
  type DiscordReplyOverride,
  type GeneralSettings,
  Home,
  type Instructions,
  McpServer,
  ModelInfo,
  Models,
  Provider,
  ProviderApi,
  type ReplyFrame,
  type ReplyPayload,
  type RequestFrame,
  type RequestPayload,
  type RequestType,
  type ScheduleItem,
  type ScheduleState,
  Slot,
  type UsageKindBucket,
  type UsageSummary,
  type WatchItem,
  type Webhook,
  type WebhooksPayload,
  isRequestType,
  requests,
} from '@blob-1/contracts'

type ProviderRecord = Readonly<{
  id: string
  api: ProviderApi
  baseUrl: string
  apiKey: string | null
  models: ReadonlyArray<ModelInfo>
}>

type State = Readonly<{
  instructions: string | null
  autoRespond: boolean
  botEnabled: boolean
  main: Slot
  compaction: Slot | null
  utility: Slot | null
  decision: DecisionSlot | null
  providers: ReadonlyArray<ProviderRecord>
  defaultAccess: DiscordAccessEffect
  accessRules: ReadonlyArray<DiscordAccessRule>
  home: Home
  hasToken: boolean
  interactionModes: ReadonlyArray<DiscordInteractionOverride>
  defaultInteractionMode: DiscordInteractionMode
  replyModes: ReadonlyArray<DiscordReplyOverride>
  defaultReplyMode: DiscordReplyMode
  mcpServers: ReadonlyArray<McpServer>
  timezone: string
  schedules: ReadonlyArray<ScheduleItem>
  nextScheduleId: number
  checkin: CheckinConfig | null
  watches: ReadonlyArray<WatchItem>
  nextWatchId: number
  publicUrl: string | null
  webhooks: ReadonlyArray<Webhook>
  nextWebhookId: number
}>

const DEFAULT_INSTRUCTIONS = '# Identity\n\nYou are Blob-1'
const REFRESHED_MODEL_COUNT = 49
const NUMERIC_ID = /^\d+$/

const seedState = (): State => ({
  instructions: null,
  autoRespond: true,
  botEnabled: true,
  main: { provider: 'cpa1', model: 'gpt-6.1-terra', reasoning: 'medium' },
  compaction: { provider: 'cpa1', model: 'gpt-6-luna', reasoning: 'medium' },
  utility: { provider: 'cpa1', model: 'gpt-6-luna', reasoning: 'low' },
  decision: null,
  providers: [
    {
      id: 'cpa1',
      api: 'responses',
      baseUrl: 'http://127.0.0.1:8320/v1',
      apiKey: 'mock-cpa1-key',
      models: [
        { id: 'gpt-6.1-terra', context: 1050000, limit: 400000, images: true },
        { id: 'gpt-6-luna', context: 1050000, limit: null, images: true },
        { id: 'gpt-6-luna-fast', context: null, limit: null, images: false },
        { id: 'claude-fable-5', context: 1000000, limit: null, images: false },
        { id: 'gpt-5.6-terra', context: 1050000, limit: null, images: false },
      ],
    },
  ],
  defaultAccess: 'block',
  accessRules: [{ userId: '435482941515104257', scope: 'global', scopeId: null, effect: 'allow' }],
  home: { kind: 'none' },
  hasToken: true,
  interactionModes: [],
  defaultInteractionMode: 'auto',
  replyModes: [],
  defaultReplyMode: 'auto',
  mcpServers: [
    {
      id: 'filesystem',
      transport: 'stdio',
      command: 'npx',
      args: ['-y', '@modelcontextprotocol/server-filesystem', '/tmp'],
      enabled: true,
      auth: 'none',
      status: 'connected',
      toolCount: 11,
    },
  ],
  timezone: 'Asia/Bangkok',
  schedules: [
    {
      id: 's1',
      text: 'Summarize unread GitHub notifications and PR reviews.',
      kind: 'cron',
      when: '0 9 * * 1-5',
      timezone: 'Asia/Bangkok',
      nextDue: Math.floor(Date.now() / 1000) + 3600,
      target: null,
    },
  ],
  nextScheduleId: 2,
  checkin: {
    every: 1800,
    days: ['mon', 'tue', 'wed', 'thu', 'fri'],
    startHour: 9,
    endHour: 22,
  },
  watches: [
    {
      id: 'w1',
      text: 'Check if blob-1 release workflow finished.',
      until: Math.floor(Date.now() / 1000) + 86400,
      target: null,
    },
  ],
  nextWatchId: 2,
  publicUrl: 'https://blob.example.com',
  webhooks: [
    {
      id: 'wh1',
      name: 'GitHub deploys',
      token: 'whk_8f3a9c2d1e4b7a6f0d5c8e2a9b1f4c7d',
      enabled: true,
      createdAt: Math.floor(Date.now() / 1000) - 86400 * 12,
      lastTriggeredAt: Math.floor(Date.now() / 1000) - 1420,
    },
    {
      id: 'wh2',
      name: 'Linear triage',
      token: 'whk_3e7b1d9f4a2c8e6b0f5a3d7c9e1b4f8a',
      enabled: false,
      createdAt: Math.floor(Date.now() / 1000) - 86400 * 4,
      lastTriggeredAt: null,
    },
  ],
  nextWebhookId: 3,
})

const generalReply = (state: State): GeneralSettings => ({
  autoRespond: state.autoRespond,
  hasUtilityModel: state.utility !== null || state.decision !== null,
})

const instructionsReply = (state: State): Instructions => ({
  text: state.instructions ?? DEFAULT_INSTRUCTIONS,
  isDefault: state.instructions === null,
})

const modelsReply = (state: State): Models => ({
  main: state.main,
  compaction: state.compaction,
  utility: state.utility,
  decision: state.decision,
  providers: state.providers.map(provider => ({
    id: provider.id,
    api: provider.api,
    models: provider.models,
  })),
})

const providersReply = (state: State): ReadonlyArray<Provider> =>
  state.providers.map(provider => ({
    id: provider.id,
    api: provider.api,
    baseUrl: provider.baseUrl,
    apiKey: provider.apiKey !== null ? 'set' : 'missing',
    modelCount: provider.models.length,
  }))

const scheduleReply = (state: State): ScheduleState => ({
  timezone: state.timezone,
  schedules: state.schedules,
  checkin: state.checkin,
  nextCheckin: state.checkin === null ? null : Math.floor(Date.now() / 1000) + state.checkin.every,
  watches: state.watches,
})

const webhooksReply = (state: State): WebhooksPayload => ({
  publicUrl: state.publicUrl,
  webhooks: state.webhooks,
})

const usageReply = (days: number): UsageSummary => {
  const safeDays = Math.max(1, Math.min(90, Math.round(days)))
  const scale = safeDays === 1 ? 1 : safeDays <= 7 ? 5.6 : safeDays * 0.72
  const scaled = (value: number) => Math.max(1, Math.round(value * scale))

  const chat: UsageKindBucket = {
    kind: 'chat',
    calls: scaled(14),
    inputTokens: scaled(42_600),
    cachedTokens: scaled(31_400),
    outputTokens: scaled(6_200),
  }
  const task: UsageKindBucket = {
    kind: 'task',
    calls: scaled(9),
    inputTokens: scaled(58_200),
    cachedTokens: scaled(46_800),
    outputTokens: scaled(7_900),
  }
  const utility: UsageKindBucket = {
    kind: 'utility',
    calls: scaled(11),
    inputTokens: scaled(9_400),
    cachedTokens: scaled(3_200),
    outputTokens: scaled(1_350),
  }
  const compaction: UsageKindBucket = {
    kind: 'compaction',
    calls: scaled(2),
    inputTokens: scaled(14_300),
    cachedTokens: scaled(11_100),
    outputTokens: scaled(1_850),
  }
  const byKind = [chat, task, utility, compaction]

  const byModel = [
    {
      provider: 'cpa1',
      model: 'gpt-6.1-terra',
      calls: chat.calls + task.calls,
      inputTokens: chat.inputTokens + task.inputTokens,
      cachedTokens: chat.cachedTokens + task.cachedTokens,
      outputTokens: chat.outputTokens + task.outputTokens,
    },
    {
      provider: 'cpa1',
      model: 'gpt-6-luna',
      calls: utility.calls + compaction.calls,
      inputTokens: utility.inputTokens + compaction.inputTokens,
      cachedTokens: utility.cachedTokens + compaction.cachedTokens,
      outputTokens: utility.outputTokens + compaction.outputTokens,
    },
  ]

  const totals = byKind.reduce(
    (acc, bucket) => ({
      calls: acc.calls + bucket.calls,
      inputTokens: acc.inputTokens + bucket.inputTokens,
      cachedTokens: acc.cachedTokens + bucket.cachedTokens,
      outputTokens: acc.outputTokens + bucket.outputTokens,
    }),
    { calls: 0, inputTokens: 0, cachedTokens: 0, outputTokens: 0 },
  )

  const dailyCount = Math.min(safeDays, 30)
  const today = new Date()
  const daily = Array.makeBy(dailyCount, index => {
    const d = new Date(today)
    d.setUTCDate(today.getUTCDate() - (dailyCount - 1 - index))
    const weight = 0.75 + ((index * 17) % 10) / 20
    const denom = dailyCount
    return {
      day: d.toISOString().slice(0, 10),
      calls: Math.max(1, Math.round((totals.calls / denom) * weight)),
      inputTokens: Math.round((totals.inputTokens / denom) * weight),
      cachedTokens: Math.round((totals.cachedTokens / denom) * weight),
      outputTokens: Math.round((totals.outputTokens / denom) * weight),
    }
  })

  return { days: safeDays, totals, byKind, byModel, daily }
}

const makeWebhookToken = (seed: number): string =>
  `whk_${crypto.randomUUID().replaceAll('-', '').slice(0, 28)}${String(seed).padStart(4, '0')}`

const LOCAL_DATETIME = /^(\d{4})-(\d{2})-(\d{2})[ T](\d{2}):(\d{2})$/

const parseDateInTimeZone = (raw: string, timeZone: string): number | null => {
  const trimmed = raw.trim()
  const localMatch = LOCAL_DATETIME.exec(trimmed)
  if (localMatch === null) {
    const parsed = Date.parse(trimmed)
    return Number.isNaN(parsed) ? null : Math.floor(parsed / 1000)
  }
  const [, y, mo, d, h, mi] = localMatch
  const target = {
    year: Number(y),
    month: Number(mo),
    day: Number(d),
    hour: Number(h),
    minute: Number(mi),
  }
  const targetUtc = Date.UTC(target.year, target.month - 1, target.day, target.hour, target.minute)
  const formatter = new Intl.DateTimeFormat('en-US', {
    timeZone,
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    hourCycle: 'h23',
  })
  const partsAt = (ms: number) => {
    const parts = formatter.formatToParts(new Date(ms))
    const get = (type: Intl.DateTimeFormatPartTypes) =>
      Number(parts.find(part => part.type === type)?.value ?? 0)
    return {
      year: get('year'),
      month: get('month'),
      day: get('day'),
      hour: get('hour') % 24,
      minute: get('minute'),
    }
  }
  const matchesTarget = (ms: number) => {
    const p = partsAt(ms)
    return (
      p.year === target.year &&
      p.month === target.month &&
      p.day === target.day &&
      p.hour === target.hour &&
      p.minute === target.minute
    )
  }
  const offsetAt = (ms: number): number => {
    const p = partsAt(ms)
    const rendered = Date.UTC(p.year, p.month - 1, p.day, p.hour, p.minute)
    return ms - rendered
  }
  // Probe the zone's actual offsets before and after the target day (handles
  // arbitrary DST shifts, including 2-hour transitions like Antarctica/Troll).
  const candidateOffsets = new Set<number>([
    offsetAt(targetUtc - 86_400_000),
    offsetAt(targetUtc),
    offsetAt(targetUtc + 86_400_000),
  ])
  const matches = new Set<number>()
  for (const offset of candidateOffsets) {
    const candidate = targetUtc + offset
    if (matchesTarget(candidate)) {
      matches.add(candidate)
    }
  }
  // Require exactly one matching UTC instant (rejects both spring-forward gaps
  // and fall-back ambiguous times, matching Rust `LocalResult::Single`).
  if (matches.size !== 1) return null
  const [resolved] = matches
  return resolved === undefined ? null : Math.floor(resolved / 1000)
}

const discordReply = (state: State): Discord => ({
  allowed: state.accessRules
    .filter(rule => rule.userId !== null && rule.scope === 'global' && rule.effect === 'allow')
    .map(rule => rule.userId ?? ''),
  defaultAccess: state.defaultAccess,
  accessRules: state.accessRules,
  home: state.home,
  token: state.hasToken ? 'set' : 'missing',
  connection:
    state.hasToken && state.botEnabled ? { state: 'online', bot: 'Blob-1' } : { state: 'off' },
  interactionModes: state.interactionModes,
  defaultInteractionMode: state.defaultInteractionMode,
  replyModes: state.replyModes,
  defaultReplyMode: state.defaultReplyMode,
})

const setToken = (state: State, token: string): Effect.Effect<State, string> =>
  token.trim() === ''
    ? Effect.fail('Token is required')
    : token.trim().length < 20
      ? Effect.fail('Discord rejected this token')
      : Effect.succeed(Struct.evolve(state, { hasToken: () => true }))

const hasModel = (state: State, providerId: string, modelId: string): boolean =>
  state.providers.some(
    provider => provider.id === providerId && provider.models.some(model => model.id === modelId),
  )

const isCloudflareAiUrl = (baseUrl: string): boolean => {
  try {
    const parsed = new URL(baseUrl.trim())
    const path = parsed.pathname.replace(/\/+$/, '').toLowerCase()
    return (
      parsed.hostname.toLowerCase() === 'api.cloudflare.com' &&
      path.startsWith('/client/v4/accounts/') &&
      path.endsWith('/ai/run')
    )
  } catch {
    return false
  }
}

const cloudflareDecisionModels = (): ReadonlyArray<ModelInfo> => [
  { id: '@cf/cloudflare/clef-flash', context: 24576, limit: null, images: false },
  { id: '@cf/cloudflare/clef', context: 65536, limit: null, images: false },
]

const hasLlmModel = (state: State, provider: string, model: string): boolean => {
  const entry = state.providers.find(p => p.id === provider)
  return (
    entry !== undefined && entry.api !== 'decision' && entry.models.some(info => info.id === model)
  )
}

const hasDecisionModel = (state: State, provider: string, model: string): boolean => {
  const entry = state.providers.find(p => p.id === provider)
  return (
    entry !== undefined && entry.api === 'decision' && entry.models.some(info => info.id === model)
  )
}

const checkSlot = (state: State, slot: Slot | null): Effect.Effect<Slot | null, string> =>
  slot === null || hasLlmModel(state, slot.provider, slot.model)
    ? Effect.succeed(slot)
    : Effect.fail('Unknown LLM model')

const checkDecisionSlot = (
  state: State,
  slot: DecisionSlot | null,
): Effect.Effect<DecisionSlot | null, string> =>
  slot === null || hasDecisionModel(state, slot.provider, slot.model)
    ? Effect.succeed(slot)
    : Effect.fail('Only decision providers can be used in the Decision slot')

const synthesizeModels = (providerId: string): ReadonlyArray<ModelInfo> =>
  Array.makeBy(REFRESHED_MODEL_COUNT, index => ({
    id: `${providerId}-model-${String(index + 1).padStart(2, '0')}`,
    context: null,
    limit: null,
    images: false,
  }))

const hasValidAuthority = (authority: string): boolean => {
  const hostPort = authority.slice(authority.indexOf('@') + 1)
  return (
    /^[\x21-\x7e]+$/.test(authority) && !/[\\<>"{}|^`]/.test(authority) && !hostPort.includes('%')
  )
}

const hasValidPathAndQuery = (raw: string): boolean => {
  const q = raw.indexOf('?')
  const path = q === -1 ? raw : raw.slice(0, q)
  const query = q === -1 ? '' : raw.slice(q + 1)
  return !/[<>`]/.test(path) && !/[<>"]/.test(query)
}

const isValidHttpUrl = (baseUrl: string): boolean => {
  if (/[\s\p{Cc}]/u.test(baseUrl)) return false
  const match = /^https?:\/\/([^/?#]+)([^#]*)(?:#.*)?$/i.exec(baseUrl)
  if (
    match === null ||
    !hasValidAuthority(match[1] ?? '') ||
    !hasValidPathAndQuery(match[2] ?? '')
  ) {
    return false
  }
  try {
    const parsed = new URL(baseUrl)
    return (parsed.protocol === 'http:' || parsed.protocol === 'https:') && parsed.hostname !== ''
  } catch {
    return false
  }
}

const resolveInitialModels = (
  api: ProviderApi,
  baseUrl: string,
  models: ReadonlyArray<string> | undefined,
): Effect.Effect<ReadonlyArray<ModelInfo>, string> => {
  if (api !== 'decision') return Effect.succeed([])
  const explicit = (models ?? [])
    .map(modelId => modelId.trim())
    .filter(modelId => modelId !== '')
    .map(modelId => ({ id: modelId, context: null, limit: null, images: false }))
  if (explicit.length > 0) return Effect.succeed(explicit)
  if (isCloudflareAiUrl(baseUrl)) return Effect.succeed(cloudflareDecisionModels())
  return Effect.fail('Enter at least one model ID for non-Cloudflare decision endpoints')
}

const addProvider = (
  state: State,
  payload: RequestPayload<'providers.add'>,
): Effect.Effect<State, string> =>
  Effect.gen(function* () {
    const id = payload.id.trim()
    const baseUrl = payload.baseUrl.trim()
    const apiKey =
      payload.apiKey === null || payload.apiKey.trim() === '' ? null : payload.apiKey.trim()

    if (id === '' || baseUrl === '') {
      return yield* Effect.fail('ID and base URL are required')
    }
    if (id.includes('/')) {
      return yield* Effect.fail("Provider ID cannot contain '/'")
    }
    if (state.providers.some(provider => provider.id === id)) {
      return yield* Effect.fail(`Provider ${id} already exists`)
    }
    if (!isValidHttpUrl(baseUrl)) {
      return yield* Effect.fail('Base URL must be a valid http(s) URL')
    }

    const seededModels = yield* resolveInitialModels(payload.api, baseUrl, payload.models)
    return Struct.evolve(state, {
      providers: providers =>
        Array.append(providers, { id, api: payload.api, baseUrl, apiKey, models: seededModels }),
    })
  })

const setProviderKey = (
  state: State,
  payload: RequestPayload<'providers.setKey'>,
): Effect.Effect<State, string> =>
  Effect.gen(function* () {
    const { id } = payload
    const apiKey =
      payload.apiKey === null || payload.apiKey.trim() === '' ? null : payload.apiKey.trim()
    if (!state.providers.some(provider => provider.id === id)) {
      return yield* Effect.fail(`Unknown provider ${id}`)
    }
    return Struct.evolve(state, {
      providers: providers =>
        providers.map(provider => (provider.id === id ? { ...provider, apiKey } : provider)),
    })
  })

const removeProvider = (state: State, id: string): Effect.Effect<State, string> =>
  Effect.gen(function* () {
    if (!state.providers.some(provider => provider.id === id)) {
      return yield* Effect.fail(`Unknown provider ${id}`)
    }
    const isInUse = [state.main, state.compaction, state.utility, state.decision].some(
      slot => slot?.provider === id,
    )
    if (isInUse) {
      return yield* Effect.fail(`Provider ${id} is used by a model slot`)
    }

    return Struct.evolve(state, {
      providers: providers => Array.filter(providers, provider => provider.id !== id),
    })
  })

const refreshProvider = (state: State, id: string): Effect.Effect<State, string> =>
  Effect.gen(function* () {
    const provider = state.providers.find(entry => entry.id === id)
    if (provider === undefined) {
      return yield* Effect.fail(`Unknown provider ${id}`)
    }

    const models =
      provider.api === 'decision'
        ? isCloudflareAiUrl(provider.baseUrl)
          ? [
              ...cloudflareDecisionModels(),
              ...provider.models.filter(
                existing => !cloudflareDecisionModels().some(cf => cf.id === existing.id),
              ),
            ]
          : provider.models
        : provider.models.length === 0
          ? synthesizeModels(id)
          : provider.models

    return Struct.evolve(state, {
      providers: providers =>
        providers.map(entry =>
          entry.id === id ? Struct.evolve(entry, { models: () => models }) : entry,
        ),
    })
  })

const allowUser = (state: State, user: string): Effect.Effect<State, string> =>
  Effect.gen(function* () {
    if (!NUMERIC_ID.test(user)) {
      return yield* Effect.fail('User id must be numeric')
    }
    const rule: DiscordAccessRule = {
      userId: user,
      scope: 'global',
      scopeId: null,
      effect: 'allow',
    }
    if (
      state.accessRules.some(
        existing => sameAccessRuleKey(existing, rule) && existing.effect === 'allow',
      )
    ) {
      return yield* Effect.fail('User is already allowed')
    }

    return Struct.evolve(state, {
      accessRules: rules =>
        sortedAccessRules([...rules.filter(existing => !sameAccessRuleKey(existing, rule)), rule]),
    })
  })

const disallowUser = (state: State, user: string): Effect.Effect<State, string> =>
  Effect.gen(function* () {
    if (!NUMERIC_ID.test(user)) {
      return yield* Effect.fail('User id must be numeric')
    }
    const key = { userId: user, scope: 'global' as const, scopeId: null }
    if (
      !state.accessRules.some(
        existing => sameAccessRuleKey(existing, key) && existing.effect === 'allow',
      )
    ) {
      return yield* Effect.fail('User is not allowed')
    }

    return Struct.evolve(state, {
      accessRules: rules =>
        rules.filter(
          existing => !(sameAccessRuleKey(existing, key) && existing.effect === 'allow'),
        ),
    })
  })

const setHome = (state: State, home: Home): Effect.Effect<State, string> =>
  Effect.gen(function* () {
    if (home.kind !== 'none' && !NUMERIC_ID.test(home.kind === 'channel' ? home.id : home.user)) {
      return yield* Effect.fail('Channel and user ids must be numeric')
    }

    return Struct.evolve(state, { home: () => home })
  })

const SNOWFLAKE = /^\d{17,20}$/

const sortedModes = (
  modes: ReadonlyArray<DiscordInteractionOverride>,
): ReadonlyArray<DiscordInteractionOverride> =>
  modes.toSorted(
    (a, b) =>
      a.scope.localeCompare(b.scope) ||
      (a.guildId ?? '').localeCompare(b.guildId ?? '') ||
      a.targetId.localeCompare(b.targetId),
  )

const sameOverrideKey = (
  entry: DiscordInteractionOverride,
  key: {
    readonly scope: DiscordInteractionOverride['scope']
    readonly guildId: string | null
    readonly targetId: string
  },
): boolean =>
  entry.scope === key.scope && entry.guildId === key.guildId && entry.targetId === key.targetId

const setInteractionMode = (
  state: State,
  payload: RequestPayload<'discord.setInteractionMode'>,
): Effect.Effect<State, string> =>
  Effect.gen(function* () {
    if (!SNOWFLAKE.test(payload.targetId)) {
      return yield* Effect.fail('Ids must be 17–20 digits')
    }
    if (payload.scope !== 'user' && payload.guildId !== null) {
      return yield* Effect.fail('A server id applies only to user overrides')
    }
    if (
      payload.scope === 'user' &&
      (payload.guildId === null || !SNOWFLAKE.test(payload.guildId))
    ) {
      return yield* Effect.fail('Server IDs must be 17–20 digits')
    }

    return Struct.evolve(state, {
      interactionModes: modes =>
        sortedModes([...modes.filter(entry => !sameOverrideKey(entry, payload)), payload]),
    })
  })

const clearInteractionMode = (
  state: State,
  payload: RequestPayload<'discord.clearInteractionMode'>,
): Effect.Effect<State, string> =>
  Effect.gen(function* () {
    if (payload.targetId === '') {
      return yield* Effect.fail('Invalid target id')
    }
    if (payload.scope !== 'user' && payload.guildId !== null) {
      return yield* Effect.fail('Invalid payload')
    }

    return Struct.evolve(state, {
      interactionModes: modes => modes.filter(entry => !sameOverrideKey(entry, payload)),
    })
  })

const sortedReplyModes = (
  modes: ReadonlyArray<DiscordReplyOverride>,
): ReadonlyArray<DiscordReplyOverride> =>
  modes.toSorted((a, b) => a.scope.localeCompare(b.scope) || a.targetId.localeCompare(b.targetId))

const sameReplyKey = (
  entry: DiscordReplyOverride,
  key: {
    readonly scope: DiscordReplyOverride['scope']
    readonly targetId: string
  },
): boolean => entry.scope === key.scope && entry.targetId === key.targetId

const setReplyMode = (
  state: State,
  payload: RequestPayload<'discord.setReplyMode'>,
): Effect.Effect<State, string> =>
  Effect.gen(function* () {
    if (!SNOWFLAKE.test(payload.targetId)) {
      return yield* Effect.fail('Ids must be 17–20 digits')
    }

    return Struct.evolve(state, {
      replyModes: modes =>
        sortedReplyModes([...modes.filter(entry => !sameReplyKey(entry, payload)), payload]),
    })
  })

const clearReplyMode = (
  state: State,
  payload: RequestPayload<'discord.clearReplyMode'>,
): Effect.Effect<State, string> =>
  Effect.gen(function* () {
    if (!SNOWFLAKE.test(payload.targetId)) {
      return yield* Effect.fail('Ids must be 17–20 digits')
    }

    return Struct.evolve(state, {
      replyModes: modes => modes.filter(entry => !sameReplyKey(entry, payload)),
    })
  })

const accessScopeOrder = { channel: 0, server: 1, global: 2 } as const

const sortedAccessRules = (
  rules: ReadonlyArray<DiscordAccessRule>,
): ReadonlyArray<DiscordAccessRule> =>
  rules.toSorted(
    (a, b) =>
      Number(a.userId === null) - Number(b.userId === null) ||
      accessScopeOrder[a.scope] - accessScopeOrder[b.scope] ||
      (a.userId ?? '').localeCompare(b.userId ?? '') ||
      (a.scopeId ?? '').localeCompare(b.scopeId ?? ''),
  )

const sameAccessRuleKey = (
  rule: DiscordAccessRule,
  key: {
    readonly userId: string | null
    readonly scope: DiscordAccessRule['scope']
    readonly scopeId: string | null
  },
): boolean => rule.userId === key.userId && rule.scope === key.scope && rule.scopeId === key.scopeId

const setAccessRule = (
  state: State,
  payload: RequestPayload<'discord.setAccessRule'>,
): Effect.Effect<State, string> =>
  Effect.gen(function* () {
    if (payload.userId !== null && !SNOWFLAKE.test(payload.userId)) {
      return yield* Effect.fail('User IDs must be 17–20 digits')
    }
    if (payload.scope === 'global') {
      if (payload.userId === null || payload.scopeId !== null) {
        return yield* Effect.fail('Global access rules require a user and no scope id')
      }
    }
    if (
      payload.scope === 'server' &&
      (payload.scopeId === null || !SNOWFLAKE.test(payload.scopeId))
    ) {
      return yield* Effect.fail('Server IDs must be 17–20 digits')
    }
    if (
      payload.scope === 'channel' &&
      (payload.scopeId === null || !SNOWFLAKE.test(payload.scopeId))
    ) {
      return yield* Effect.fail('Channel IDs must be 17–20 digits')
    }

    return Struct.evolve(state, {
      accessRules: rules =>
        sortedAccessRules([...rules.filter(rule => !sameAccessRuleKey(rule, payload)), payload]),
    })
  })

const removeAccessRule = (
  state: State,
  payload: RequestPayload<'discord.removeAccessRule'>,
): Effect.Effect<State, string> =>
  Effect.gen(function* () {
    if (payload.userId === '') {
      return yield* Effect.fail('Invalid user id')
    }
    if (payload.scope === 'global' && (payload.userId === null || payload.scopeId !== null)) {
      return yield* Effect.fail('Invalid payload')
    }
    if (payload.scope !== 'global' && (payload.scopeId === null || payload.scopeId === '')) {
      return yield* Effect.fail('Invalid scope id')
    }

    return Struct.evolve(state, {
      accessRules: rules => rules.filter(rule => !sameAccessRuleKey(rule, payload)),
    })
  })

const MCP_ID = /^[a-z0-9-]+$/

const addMcpServer = (
  state: State,
  payload: RequestPayload<'mcp.add'>,
): Effect.Effect<State, string> =>
  Effect.gen(function* () {
    const { id } = payload
    if (!MCP_ID.test(id)) {
      return yield* Effect.fail('Invalid id: use lowercase letters, digits and -')
    }
    if (state.mcpServers.some(server => server.id === id)) {
      return yield* Effect.fail(`MCP server ${id} already exists`)
    }
    const detail = payload.transport === 'stdio' ? payload.command : payload.url
    if (detail.trim() === '') {
      return yield* Effect.fail(
        payload.transport === 'stdio' ? 'Command is required' : 'URL is required',
      )
    }

    const server: McpServer =
      payload.transport === 'stdio'
        ? {
            id,
            transport: 'stdio',
            command: payload.command,
            args: payload.args ?? [],
            enabled: true,
            auth: 'none',
            status: 'idle',
          }
        : {
            id,
            transport: 'http',
            url: payload.url,
            enabled: true,
            auth: 'oauth',
            status: 'needs-login',
          }

    return Struct.evolve(state, {
      mcpServers: servers => Array.append(servers, server),
    })
  })

const transition = (
  state: Ref.Ref<State>,
  step: (current: State) => Effect.Effect<State, string>,
): Effect.Effect<State, string> =>
  Ref.get(state).pipe(
    Effect.flatMap(step),
    Effect.tap(next => Ref.set(state, next)),
  )

const DEFAULT_BOT_ID = 'default'
const VALID_BOT_ID = /^[a-z0-9][a-z0-9-]{0,31}$/

type BotRecord = Readonly<{
  id: string
  name: string
  enabled: boolean
  createdAt: number
  generation: number
  disabledMcp: ReadonlyArray<string>
  state: State
}>

type MockRoot = Readonly<{
  providers: ReadonlyArray<ProviderRecord>
  mcpServers: ReadonlyArray<McpServer>
  bots: ReadonlyArray<BotRecord>
  /** Monotonic instance id so a recreated slug never reuses a prior generation. */
  nextGeneration: number
}>

const emptyBotState = (name: string, base: State): State => ({
  ...base,
  instructions: `# Identity\n\nYou are ${name}`,
  autoRespond: true,
  main: { provider: '', model: '', reasoning: 'medium' },
  compaction: null,
  utility: null,
  decision: null,
  defaultAccess: 'block',
  accessRules: [],
  home: { kind: 'none' },
  hasToken: false,
  interactionModes: [],
  defaultInteractionMode: 'mention-only',
  replyModes: [],
  defaultReplyMode: 'auto',
  schedules: [],
  nextScheduleId: 1,
  checkin: null,
  watches: [],
  nextWatchId: 1,
  webhooks: [],
  nextWebhookId: 1,
})

const seedRoot = (): MockRoot => {
  const initial = seedState()
  return {
    providers: initial.providers,
    mcpServers: initial.mcpServers,
    bots: [
      {
        id: DEFAULT_BOT_ID,
        name: 'Blob-1',
        enabled: true,
        createdAt: 0,
        generation: 0,
        disabledMcp: [],
        state: initial,
      },
    ],
    nextGeneration: 1,
  }
}

const botsReply = (root: MockRoot): ReadonlyArray<BotSummary> =>
  root.bots.map(bot => ({
    id: bot.id,
    name: bot.name,
    enabled: bot.enabled,
    isDefault: bot.id === DEFAULT_BOT_ID,
    createdAt: bot.createdAt,
    generation: bot.generation,
    mainModel:
      bot.state.main.provider !== '' && bot.state.main.model !== ''
        ? `${bot.state.main.provider}/${bot.state.main.model}`
        : null,
    connection:
      bot.enabled && bot.state.hasToken ? { state: 'online', bot: bot.name } : { state: 'off' },
  }))

const botMcpReply = (root: MockRoot, botId: string): Effect.Effect<BotMcpState, string> => {
  const bot = root.bots.find(entry => entry.id === botId)
  if (bot === undefined) return Effect.fail(`Unknown bot ${botId}`)
  return Effect.succeed({
    servers: root.mcpServers.map(server => ({
      id: server.id,
      enabledGlobal: server.enabled,
      enabledForBot: !bot.disabledMcp.includes(server.id),
    })),
  })
}

const providerInUseByBot = (root: MockRoot, providerId: string): string | null => {
  for (const bot of root.bots) {
    const s = bot.state
    if (
      s.main.provider === providerId ||
      s.compaction?.provider === providerId ||
      s.utility?.provider === providerId ||
      s.decision?.provider === providerId
    ) {
      return bot.id
    }
  }
  return null
}

type LegacyRequestType = Exclude<
  RequestType,
  'bots.list' | 'bots.create' | 'bots.update' | 'bots.remove' | 'bot.mcp.get' | 'bot.mcp.setEnabled'
>

type Handler<K extends LegacyRequestType> = (
  payload: RequestPayload<K>,
  state: Ref.Ref<State>,
) => Effect.Effect<ReplyPayload<K>, string>

// oxlint-disable-next-line anti-slop/no-known-value-widening -- the mapped type keeps each handler tied to its request type for generic dispatch
const handlers: { readonly [K in LegacyRequestType]: Handler<K> } = {
  'general.get': (_payload, state) => Ref.get(state).pipe(Effect.map(generalReply)),

  'general.setAutoRespond': ({ enabled }, state) =>
    transition(state, current =>
      Effect.succeed(Struct.evolve(current, { autoRespond: () => enabled })),
    ).pipe(Effect.map(generalReply)),

  'instructions.get': (_payload, state) => Ref.get(state).pipe(Effect.map(instructionsReply)),

  'instructions.set': ({ text }, state) =>
    transition(state, current =>
      text.trim() === ''
        ? Effect.fail('Instructions cannot be blank')
        : Effect.succeed(Struct.evolve(current, { instructions: () => text })),
    ).pipe(Effect.map(instructionsReply)),

  'instructions.reset': (_payload, state) =>
    transition(state, current =>
      Effect.succeed(Struct.evolve(current, { instructions: () => null })),
    ).pipe(Effect.map(instructionsReply)),

  'models.get': (_payload, state) => Ref.get(state).pipe(Effect.map(modelsReply)),

  'models.setMain': ({ provider, model, reasoning }, state) =>
    transition(state, current =>
      hasLlmModel(current, provider, model)
        ? Effect.succeed(
            Struct.evolve(current, {
              main: () => ({ provider, model, reasoning }),
            }),
          )
        : Effect.fail('Unknown model'),
    ).pipe(Effect.map(modelsReply)),

  'models.setCompaction': ({ slot }, state) =>
    transition(state, current =>
      checkSlot(current, slot).pipe(
        Effect.map(next => Struct.evolve(current, { compaction: () => next })),
      ),
    ).pipe(Effect.map(modelsReply)),

  'models.setUtility': ({ slot }, state) =>
    transition(state, current =>
      checkSlot(current, slot).pipe(
        Effect.map(next => Struct.evolve(current, { utility: () => next })),
      ),
    ).pipe(Effect.map(modelsReply)),

  'models.setDecision': ({ slot }, state) =>
    transition(state, current =>
      checkDecisionSlot(current, slot).pipe(
        Effect.map(next => Struct.evolve(current, { decision: () => next })),
      ),
    ).pipe(Effect.map(modelsReply)),

  'models.setContextLimit': ({ provider, model, limit }, state) =>
    transition(state, current =>
      limit !== null && (!Number.isInteger(limit) || limit <= 0)
        ? Effect.fail('Context limit must be a positive integer')
        : !hasModel(current, provider, model)
          ? Effect.fail('Unknown model')
          : Effect.succeed(
              Struct.evolve(current, {
                providers: providers =>
                  providers.map(entry =>
                    entry.id === provider
                      ? Struct.evolve(entry, {
                          models: models =>
                            models.map(info =>
                              info.id === model
                                ? Struct.evolve(info, { limit: () => limit })
                                : info,
                            ),
                        })
                      : entry,
                  ),
              }),
            ),
    ).pipe(Effect.map(modelsReply)),

  'models.setImages': ({ provider, model, images }, state) =>
    transition(state, current =>
      hasModel(current, provider, model)
        ? Effect.succeed(
            Struct.evolve(current, {
              providers: providers =>
                providers.map(entry =>
                  entry.id === provider
                    ? Struct.evolve(entry, {
                        models: models =>
                          models.map(entryModel =>
                            entryModel.id === model
                              ? Struct.evolve(entryModel, {
                                  images: () => images,
                                })
                              : entryModel,
                          ),
                      })
                    : entry,
                ),
            }),
          )
        : Effect.fail('Unknown model'),
    ).pipe(Effect.map(modelsReply)),

  'providers.list': (_payload, state) => Ref.get(state).pipe(Effect.map(providersReply)),

  'providers.add': (payload, state) =>
    transition(state, current => addProvider(current, payload)).pipe(Effect.map(providersReply)),

  'providers.setKey': (payload, state) =>
    transition(state, current => setProviderKey(current, payload)).pipe(Effect.map(providersReply)),

  'providers.remove': ({ id }, state) =>
    transition(state, current => removeProvider(current, id)).pipe(Effect.map(providersReply)),

  'providers.refresh': ({ id }, state) =>
    transition(state, current => refreshProvider(current, id)).pipe(Effect.map(providersReply)),

  'discord.get': (_payload, state) => Ref.get(state).pipe(Effect.map(discordReply)),

  'discord.directory': () =>
    Effect.succeed({
      guilds: [
        { id: '928368271290421000', name: 'Blob Lab' },
        { id: '842724454677348000', name: 'Studio' },
      ],
      channels: [
        {
          id: '928368271290421338',
          name: 'server-man',
          server: 'Blob Lab',
          guildId: '928368271290421000',
          kind: 'channel',
        },
        {
          id: '928368271290421339',
          name: 'general',
          server: 'Blob Lab',
          guildId: '928368271290421000',
          kind: 'channel',
        },
        {
          id: '928368271290421340',
          name: 'eng',
          server: 'Blob Lab',
          guildId: '928368271290421000',
          kind: 'channel',
        },
        {
          id: '842724454677348360',
          name: 'announcements',
          server: 'Studio',
          guildId: '842724454677348000',
          kind: 'channel',
        },
      ],
      people: [
        {
          id: '435482941515104257',
          name: 'Chan Nyein Thaw',
          guilds: ['928368271290421000', '842724454677348000'],
        },
        {
          id: '694730852973805679',
          name: 'Su Hlaing Hnin',
          guilds: ['928368271290421000'],
        },
        {
          id: '1090496556781809745',
          name: 'Pinn',
          guilds: ['842724454677348000'],
        },
      ],
    }),

  'discord.allow': ({ user }, state) =>
    transition(state, current => allowUser(current, user)).pipe(Effect.map(discordReply)),

  'discord.disallow': ({ user }, state) =>
    transition(state, current => disallowUser(current, user)).pipe(Effect.map(discordReply)),

  'discord.setHome': ({ home }, state) =>
    transition(state, current => setHome(current, home)).pipe(Effect.map(discordReply)),

  'discord.setToken': ({ token }, state) =>
    transition(state, current => setToken(current, token)).pipe(Effect.map(discordReply)),

  'discord.clearToken': (_payload, state) =>
    transition(state, current =>
      Effect.succeed(Struct.evolve(current, { hasToken: () => false })),
    ).pipe(Effect.map(discordReply)),

  'discord.setInteractionMode': (payload, state) =>
    transition(state, current => setInteractionMode(current, payload)).pipe(
      Effect.map(discordReply),
    ),

  'discord.clearInteractionMode': (payload, state) =>
    transition(state, current => clearInteractionMode(current, payload)).pipe(
      Effect.map(discordReply),
    ),

  'discord.setDefaultInteractionMode': ({ mode }, state) =>
    transition(state, current =>
      Effect.succeed(Struct.evolve(current, { defaultInteractionMode: () => mode })),
    ).pipe(Effect.map(discordReply)),

  'discord.setReplyMode': (payload, state) =>
    transition(state, current => setReplyMode(current, payload)).pipe(Effect.map(discordReply)),

  'discord.clearReplyMode': (payload, state) =>
    transition(state, current => clearReplyMode(current, payload)).pipe(Effect.map(discordReply)),

  'discord.setDefaultReplyMode': ({ mode }, state) =>
    transition(state, current =>
      Effect.succeed(Struct.evolve(current, { defaultReplyMode: () => mode })),
    ).pipe(Effect.map(discordReply)),

  'discord.setDefaultAccess': ({ effect }, state) =>
    transition(state, current =>
      Effect.succeed(Struct.evolve(current, { defaultAccess: () => effect })),
    ).pipe(Effect.map(discordReply)),

  'discord.setAccessRule': (payload, state) =>
    transition(state, current => setAccessRule(current, payload)).pipe(Effect.map(discordReply)),

  'discord.removeAccessRule': (payload, state) =>
    transition(state, current => removeAccessRule(current, payload)).pipe(Effect.map(discordReply)),

  'mcp.list': (_payload, state) => Ref.get(state).pipe(Effect.map(current => current.mcpServers)),

  'mcp.add': (payload, state) =>
    transition(state, current => addMcpServer(current, payload)).pipe(
      Effect.map(current => current.mcpServers),
    ),

  'mcp.remove': ({ id }, state) =>
    transition(state, current =>
      current.mcpServers.some(server => server.id === id)
        ? Effect.succeed(
            Struct.evolve(current, {
              mcpServers: servers => Array.filter(servers, server => server.id !== id),
            }),
          )
        : Effect.fail(`Unknown MCP server ${id}`),
    ).pipe(Effect.map(current => current.mcpServers)),

  'mcp.setEnabled': ({ id, enabled }, state) =>
    transition(state, current =>
      current.mcpServers.some(server => server.id === id)
        ? Effect.succeed(
            Struct.evolve(current, {
              mcpServers: servers =>
                servers.map(server =>
                  server.id === id
                    ? Struct.evolve(server, {
                        enabled: () => enabled,
                        status: () => (enabled ? 'idle' : 'disconnected'),
                      })
                    : server,
                ),
            }),
          )
        : Effect.fail(`Unknown MCP server ${id}`),
    ).pipe(Effect.map(current => current.mcpServers)),

  'mcp.login': ({ id }, state) =>
    Ref.get(state).pipe(
      Effect.flatMap(current =>
        current.mcpServers.some(server => server.id === id && server.transport === 'http')
          ? Effect.succeed({ url: `/settings/mcp?logged-in=${id}` })
          : Effect.fail(`MCP server '${id}' is not an HTTP server`),
      ),
    ),

  'schedule.get': (_payload, state) => Ref.get(state).pipe(Effect.map(scheduleReply)),

  'schedule.occurrences': ({ start, end, cursor }, state) =>
    Ref.get(state).pipe(
      Effect.map(current => {
        const items = current.schedules
          .filter(item => item.nextDue >= start && item.nextDue <= end)
          .map(item => ({ scheduleId: item.id, at: item.nextDue }))
          .toSorted((a, b) => a.at - b.at || a.scheduleId.localeCompare(b.scheduleId))
          .filter(
            item =>
              cursor === undefined ||
              cursor === null ||
              item.at > cursor.at ||
              (item.at === cursor.at && item.scheduleId > cursor.scheduleId),
          )
        return { occurrences: items, nextCursor: null }
      }),
    ),

  'schedule.setTimezone': ({ timezone }, state) =>
    transition(state, current =>
      timezone.trim() === ''
        ? Effect.fail('Invalid timezone')
        : Effect.succeed(Struct.evolve(current, { timezone: () => timezone.trim() })),
    ).pipe(Effect.map(scheduleReply)),

  'schedule.add': (payload, state) =>
    transition(state, current =>
      Effect.gen(function* () {
        if (payload.text.trim() === '') return yield* Effect.fail('Task is required')
        if (payload.when.trim() === '') return yield* Effect.fail('Schedule time is required')
        const tz = payload.timezone ?? current.timezone
        const now = Math.floor(Date.now() / 1000)
        const nextDue = payload.kind === 'at' ? parseDateInTimeZone(payload.when, tz) : now + 3600
        if (nextDue === null) return yield* Effect.fail('Invalid schedule time')
        if (payload.kind === 'at' && nextDue <= now) {
          return yield* Effect.fail('at must be in the future')
        }
        const id = `s${current.nextScheduleId}`
        const item: ScheduleItem = {
          id,
          text: payload.text.trim(),
          kind: payload.kind,
          when: payload.when.trim(),
          timezone: tz,
          nextDue,
          target: null,
        }
        return Struct.evolve(current, {
          schedules: list => [...list, item],
          nextScheduleId: n => n + 1,
        })
      }),
    ).pipe(Effect.map(scheduleReply)),

  'schedule.remove': ({ id }, state) =>
    transition(state, current =>
      Effect.succeed(
        Struct.evolve(current, {
          schedules: list => list.filter(item => item.id !== id),
        }),
      ),
    ).pipe(Effect.map(scheduleReply)),

  'checkin.set': (payload, state) =>
    transition(state, current =>
      Effect.gen(function* () {
        if (payload.every < 300) return yield* Effect.fail('Interval must be at least 5 minutes')
        if (payload.days.length === 0) return yield* Effect.fail('Choose at least one day')
        if (payload.startHour >= payload.endHour || payload.endHour > 24) {
          return yield* Effect.fail('End hour must be after start hour')
        }
        return Struct.evolve(current, { checkin: () => payload })
      }),
    ).pipe(Effect.map(scheduleReply)),

  'checkin.clear': (_payload, state) =>
    transition(state, current =>
      Effect.succeed(Struct.evolve(current, { checkin: () => null })),
    ).pipe(Effect.map(scheduleReply)),

  'watch.add': (payload, state) =>
    transition(state, current =>
      Effect.gen(function* () {
        if (payload.text.trim() === '') return yield* Effect.fail('Watch text is required')
        const now = Math.floor(Date.now() / 1000)
        let until: number | null = null
        if (payload.until !== null && payload.until.trim() !== '') {
          until = parseDateInTimeZone(payload.until, current.timezone)
          if (until === null) return yield* Effect.fail('Invalid watch expiration')
          if (until <= now) return yield* Effect.fail('until must be in the future')
        }
        const id = `w${current.nextWatchId}`
        const item: WatchItem = { id, text: payload.text.trim(), until, target: null }
        return Struct.evolve(current, {
          watches: list => [...list, item],
          nextWatchId: n => n + 1,
        })
      }),
    ).pipe(Effect.map(scheduleReply)),

  'watch.remove': ({ id }, state) =>
    transition(state, current =>
      Effect.succeed(
        Struct.evolve(current, {
          watches: list => list.filter(item => item.id !== id),
        }),
      ),
    ).pipe(Effect.map(scheduleReply)),

  'usage.get': ({ days }) => Effect.succeed(usageReply(days)),

  'webhooks.list': (_payload, state) => Ref.get(state).pipe(Effect.map(webhooksReply)),

  'webhooks.create': ({ name }, state) =>
    transition(state, current => {
      const trimmed = name.trim()
      if (trimmed === '') {
        return Effect.fail('Webhook name is required')
      }
      const webhook: Webhook = {
        id: `wh${current.nextWebhookId}`,
        name: trimmed,
        token: makeWebhookToken(current.nextWebhookId),
        enabled: true,
        createdAt: Math.floor(Date.now() / 1000),
        lastTriggeredAt: null,
      }
      return Effect.succeed(
        Struct.evolve(current, {
          webhooks: list => [...list, webhook],
          nextWebhookId: n => n + 1,
        }),
      )
    }).pipe(Effect.map(webhooksReply)),

  'webhooks.setEnabled': ({ id, enabled }, state) =>
    transition(state, current =>
      current.webhooks.some(webhook => webhook.id === id)
        ? Effect.succeed(
            Struct.evolve(current, {
              webhooks: list =>
                list.map(webhook =>
                  webhook.id === id ? Struct.evolve(webhook, { enabled: () => enabled }) : webhook,
                ),
            }),
          )
        : Effect.fail(`Unknown webhook ${id}`),
    ).pipe(Effect.map(webhooksReply)),

  'webhooks.remove': ({ id }, state) =>
    transition(state, current =>
      current.webhooks.some(webhook => webhook.id === id)
        ? Effect.succeed(
            Struct.evolve(current, {
              webhooks: list => list.filter(webhook => webhook.id !== id),
            }),
          )
        : Effect.fail(`Unknown webhook ${id}`),
    ).pipe(Effect.map(webhooksReply)),
}

const replyOk = (id: string, payload: ReplyPayload<RequestType>): ReplyFrame => ({
  id,
  ok: true,
  payload,
})

const replyError = (id: string, error: string): ReplyFrame => ({
  id,
  ok: false,
  error,
})

const toggleDisabledServer = (
  list: ReadonlyArray<string>,
  id: string,
  enabled: boolean,
): ReadonlyArray<string> =>
  enabled ? list.filter(entry => entry !== id) : list.includes(id) ? list : [...list, id]

const botControlHandlers = {
  'bots.list': (_payload: RequestPayload<'bots.list'>, rootRef: Ref.Ref<MockRoot>) =>
    Ref.get(rootRef).pipe(Effect.map(botsReply)),

  'bots.create': (
    { id: rawId, name: rawName }: RequestPayload<'bots.create'>,
    rootRef: Ref.Ref<MockRoot>,
  ) =>
    Effect.gen(function* () {
      const root = yield* Ref.get(rootRef)
      const id = rawId.trim()
      const name = rawName.trim()
      if (!VALID_BOT_ID.test(id)) {
        return yield* Effect.fail(
          "Bot ID must be 1–32 lowercase letters, digits, or '-' (starting with a letter or digit)",
        )
      }
      if (id === DEFAULT_BOT_ID) return yield* Effect.fail("Bot ID 'default' is reserved")
      if (name === '' || name.length > 64) {
        return yield* Effect.fail('Bot name must be 1–64 characters')
      }
      if (root.bots.some(bot => bot.id === id)) {
        return yield* Effect.fail(`Bot ${id} already exists`)
      }
      const maxCreated = root.bots.reduce((max, bot) => Math.max(max, bot.createdAt), 0)
      const createdAt = Math.max(Date.now(), maxCreated + 1)
      const next: MockRoot = {
        ...root,
        bots: [
          ...root.bots,
          {
            id,
            name,
            enabled: true,
            createdAt,
            generation: root.nextGeneration,
            disabledMcp: [],
            state: emptyBotState(name, seedState()),
          },
        ],
        nextGeneration: root.nextGeneration + 1,
      }
      yield* Ref.set(rootRef, next)
      return botsReply(next)
    }),

  'bots.update': (
    { id, name, enabled, botGeneration }: RequestPayload<'bots.update'>,
    rootRef: Ref.Ref<MockRoot>,
  ) =>
    Effect.gen(function* () {
      const root = yield* Ref.get(rootRef)
      const existing = root.bots.find(bot => bot.id === id)
      if (existing === undefined) return yield* Effect.fail(`Unknown bot ${id}`)
      yield* checkGeneration(existing, botGeneration)
      const nextName = name !== undefined && name !== null ? name.trim() : existing.name
      if (nextName === '' || nextName.length > 64) {
        return yield* Effect.fail('Bot name must be 1–64 characters')
      }
      const nextEnabled = enabled !== undefined && enabled !== null ? enabled : existing.enabled
      const next: MockRoot = Struct.evolve(root, {
        bots: bots =>
          bots.map(bot =>
            bot.id === id
              ? Struct.evolve(bot, { name: () => nextName, enabled: () => nextEnabled })
              : bot,
          ),
      })
      yield* Ref.set(rootRef, next)
      return botsReply(next)
    }),

  'bots.remove': (
    { id, botGeneration }: RequestPayload<'bots.remove'>,
    rootRef: Ref.Ref<MockRoot>,
  ) =>
    Effect.gen(function* () {
      const root = yield* Ref.get(rootRef)
      if (id === DEFAULT_BOT_ID) return yield* Effect.fail('Cannot remove the default bot')
      const existing = root.bots.find(bot => bot.id === id)
      if (existing === undefined) return yield* Effect.fail(`Unknown bot ${id}`)
      yield* checkGeneration(existing, botGeneration)
      const next: MockRoot = Struct.evolve(root, {
        bots: bots => bots.filter(bot => bot.id !== id),
      })
      yield* Ref.set(rootRef, next)
      return botsReply(next)
    }),

  'bot.mcp.get': (
    { botId, botGeneration }: RequestPayload<'bot.mcp.get'>,
    rootRef: Ref.Ref<MockRoot>,
  ) =>
    Effect.gen(function* () {
      const root = yield* Ref.get(rootRef)
      const resolved = botId ?? DEFAULT_BOT_ID
      const bot = root.bots.find(entry => entry.id === resolved)
      if (bot !== undefined) yield* checkGeneration(bot, botGeneration)
      return yield* botMcpReply(root, resolved)
    }),

  'bot.mcp.setEnabled': (
    { botId: rawBotId, botGeneration, id, enabled }: RequestPayload<'bot.mcp.setEnabled'>,
    rootRef: Ref.Ref<MockRoot>,
  ) =>
    Effect.gen(function* () {
      const root = yield* Ref.get(rootRef)
      const botId = rawBotId ?? DEFAULT_BOT_ID
      const target = root.bots.find(bot => bot.id === botId)
      if (target === undefined) {
        return yield* Effect.fail(`Unknown bot ${botId}`)
      }
      yield* checkGeneration(target, botGeneration)
      if (!root.mcpServers.some(server => server.id === id)) {
        return yield* Effect.fail(`no MCP server '${id}'`)
      }
      const next: MockRoot = Struct.evolve(root, {
        bots: bots =>
          bots.map(bot =>
            bot.id === botId
              ? Struct.evolve(bot, { disabledMcp: list => toggleDisabledServer(list, id, enabled) })
              : bot,
          ),
      })
      yield* Ref.set(rootRef, next)
      return yield* botMcpReply(next, botId)
    }),
} satisfies {
  readonly [
    K in
      | 'bots.list'
      | 'bots.create'
      | 'bots.update'
      | 'bots.remove'
      | 'bot.mcp.get'
      | 'bot.mcp.setEnabled'
  ]: (
    payload: RequestPayload<K>,
    rootRef: Ref.Ref<MockRoot>,
  ) => Effect.Effect<ReplyPayload<K>, string>
}

const botIdOf = (payload: { readonly botId?: string | undefined }): string =>
  payload.botId !== undefined && payload.botId !== '' ? payload.botId : DEFAULT_BOT_ID

const botGenerationOf = (payload: RequestPayload<LegacyRequestType>): number | undefined =>
  'botGeneration' in payload ? payload.botGeneration : undefined

/** Rejects writes aimed at a bot instance that was deleted and recreated with the same slug. */
const checkGeneration = (
  bot: BotRecord,
  botGeneration: number | undefined,
): Effect.Effect<void, string> =>
  botGeneration === undefined || bot.generation === botGeneration
    ? Effect.void
    : Effect.fail(`Bot '${bot.id}' was replaced; reload to continue`)

const runBotScoped = <K extends LegacyRequestType>(
  rootRef: Ref.Ref<MockRoot>,
  type: K,
  decoded: RequestPayload<K>,
): Effect.Effect<ReplyPayload<K>, string> =>
  Effect.gen(function* () {
    const root = yield* Ref.get(rootRef)
    const botId = 'botId' in decoded ? botIdOf(decoded) : DEFAULT_BOT_ID
    const bot = root.bots.find(entry => entry.id === botId)
    if (bot === undefined) return yield* Effect.fail(`Unknown bot ${botId}`)
    yield* checkGeneration(bot, botGenerationOf(decoded))
    const projected: State = {
      ...bot.state,
      botEnabled: bot.enabled,
      providers: root.providers,
      mcpServers: root.mcpServers,
    }
    const tempRef = yield* Ref.make(projected)
    const reply = yield* handlers[type](decoded, tempRef)
    const updated = yield* Ref.get(tempRef)
    const nextRoot: MockRoot = {
      ...root,
      providers: updated.providers,
      mcpServers: updated.mcpServers,
      bots: root.bots.map(entry =>
        entry.id === botId ? Struct.evolve(entry, { state: () => updated }) : entry,
      ),
    }
    yield* Ref.set(rootRef, nextRoot)
    return reply
  })

const removeProviderAcrossBots = (
  payload: RequestPayload<'providers.remove'>,
  rootRef: Ref.Ref<MockRoot>,
): Effect.Effect<ReplyPayload<'providers.remove'>, string> =>
  Effect.gen(function* () {
    const root = yield* Ref.get(rootRef)
    const usedBy = providerInUseByBot(root, payload.id)
    if (usedBy !== null) {
      return yield* Effect.fail(`Provider ${payload.id} is used by a model slot on bot '${usedBy}'`)
    }
    return yield* runBotScoped(rootRef, 'providers.remove', payload)
  })

type RootHandler<K extends RequestType> = (
  payload: RequestPayload<K>,
  rootRef: Ref.Ref<MockRoot>,
) => Effect.Effect<ReplyPayload<K>, string>

const wrapLegacy =
  <K extends LegacyRequestType>(type: K): RootHandler<K> =>
  (payload, rootRef) =>
    runBotScoped(rootRef, type, payload)

// oxlint-disable-next-line anti-slop/no-known-value-widening -- mapped table keeps each handler bound to its request type
const allHandlers: { readonly [K in RequestType]: RootHandler<K> } = {
  ...botControlHandlers,
  'general.get': wrapLegacy('general.get'),
  'general.setAutoRespond': wrapLegacy('general.setAutoRespond'),
  'instructions.get': wrapLegacy('instructions.get'),
  'instructions.set': wrapLegacy('instructions.set'),
  'instructions.reset': wrapLegacy('instructions.reset'),
  'models.get': wrapLegacy('models.get'),
  'models.setMain': wrapLegacy('models.setMain'),
  'models.setCompaction': wrapLegacy('models.setCompaction'),
  'models.setUtility': wrapLegacy('models.setUtility'),
  'models.setDecision': wrapLegacy('models.setDecision'),
  'models.setContextLimit': wrapLegacy('models.setContextLimit'),
  'models.setImages': wrapLegacy('models.setImages'),
  'providers.list': wrapLegacy('providers.list'),
  'providers.add': wrapLegacy('providers.add'),
  'providers.setKey': wrapLegacy('providers.setKey'),
  'providers.remove': removeProviderAcrossBots,
  'providers.refresh': wrapLegacy('providers.refresh'),
  'discord.get': wrapLegacy('discord.get'),
  'discord.directory': wrapLegacy('discord.directory'),
  'discord.allow': wrapLegacy('discord.allow'),
  'discord.disallow': wrapLegacy('discord.disallow'),
  'discord.setHome': wrapLegacy('discord.setHome'),
  'discord.setToken': wrapLegacy('discord.setToken'),
  'discord.clearToken': wrapLegacy('discord.clearToken'),
  'discord.setInteractionMode': wrapLegacy('discord.setInteractionMode'),
  'discord.clearInteractionMode': wrapLegacy('discord.clearInteractionMode'),
  'discord.setDefaultInteractionMode': wrapLegacy('discord.setDefaultInteractionMode'),
  'discord.setReplyMode': wrapLegacy('discord.setReplyMode'),
  'discord.clearReplyMode': wrapLegacy('discord.clearReplyMode'),
  'discord.setDefaultReplyMode': wrapLegacy('discord.setDefaultReplyMode'),
  'discord.setDefaultAccess': wrapLegacy('discord.setDefaultAccess'),
  'discord.setAccessRule': wrapLegacy('discord.setAccessRule'),
  'discord.removeAccessRule': wrapLegacy('discord.removeAccessRule'),
  'mcp.list': wrapLegacy('mcp.list'),
  'mcp.add': wrapLegacy('mcp.add'),
  'mcp.remove': wrapLegacy('mcp.remove'),
  'mcp.setEnabled': wrapLegacy('mcp.setEnabled'),
  'mcp.login': wrapLegacy('mcp.login'),
  'schedule.get': wrapLegacy('schedule.get'),
  'schedule.occurrences': wrapLegacy('schedule.occurrences'),
  'schedule.setTimezone': wrapLegacy('schedule.setTimezone'),
  'schedule.add': wrapLegacy('schedule.add'),
  'schedule.remove': wrapLegacy('schedule.remove'),
  'checkin.set': wrapLegacy('checkin.set'),
  'checkin.clear': wrapLegacy('checkin.clear'),
  'watch.add': wrapLegacy('watch.add'),
  'watch.remove': wrapLegacy('watch.remove'),
  'usage.get': wrapLegacy('usage.get'),
  'webhooks.list': wrapLegacy('webhooks.list'),
  'webhooks.create': wrapLegacy('webhooks.create'),
  'webhooks.setEnabled': wrapLegacy('webhooks.setEnabled'),
  'webhooks.remove': wrapLegacy('webhooks.remove'),
}

const dispatch = <K extends RequestType>(
  rootRef: Ref.Ref<MockRoot>,
  frame: RequestFrame,
  type: K,
): Effect.Effect<ReplyFrame> =>
  // oxlint-disable-next-line effect-local/no-inline-schema-compile -- the payload schema depends on the request type
  Schema.decodeUnknownEffect(requests[type].payload)(frame.payload).pipe(
    Effect.mapError(() => `Invalid ${type} payload`),
    Effect.flatMap(decoded => allHandlers[type](decoded, rootRef)),
    Effect.map(reply => replyOk(frame.id, reply)),
    Effect.catch(error => Effect.succeed(replyError(frame.id, error))),
  )

const serve = (rootRef: Ref.Ref<MockRoot>, frame: RequestFrame): Effect.Effect<ReplyFrame> =>
  isRequestType(frame.type)
    ? dispatch(rootRef, frame, frame.type)
    : Effect.succeed(replyError(frame.id, `Unknown request ${frame.type}`))

const latency = Random.nextIntBetween(150, 351).pipe(
  Effect.flatMap(ms => Effect.sleep(Duration.millis(ms))),
)

export const mockTransport: Layer.Layer<Transport> = Layer.effect(
  Transport,
  Effect.gen(function* () {
    const rootRef = yield* Ref.make(seedRoot())

    return Transport.of({
      send: frame => latency.pipe(Effect.andThen(serve(rootRef, frame))),
      status: Stream.make(true),
      unauthorized: Stream.empty,
    })
  }),
)
