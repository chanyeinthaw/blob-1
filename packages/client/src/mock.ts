import { Array, Duration, Effect, Layer, Random, Ref, Schema, Stream, Struct } from 'effect'

import { Transport } from './api.ts'
import {
  type CheckinConfig,
  Discord,
  type DiscordAccessEntry,
  type DiscordAccessMode,
  type DiscordInteractionMode,
  type DiscordInteractionOverride,
  type DiscordReplyMode,
  type DiscordReplyOverride,
  type GeneralSettings,
  Home,
  Identity,
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
  identity: string
  autoRespond: boolean
  main: Slot
  compaction: Slot | null
  utility: Slot | null
  providers: ReadonlyArray<ProviderRecord>
  accessMode: DiscordAccessMode
  accessEntries: ReadonlyArray<DiscordAccessEntry>
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

const DEFAULT_IDENTITY = 'You are Blob-1.'
const REFRESHED_MODEL_COUNT = 49
const NUMERIC_ID = /^\d+$/

const seedState = (): State => ({
  identity: DEFAULT_IDENTITY,
  autoRespond: true,
  main: { provider: 'cpa1', model: 'gpt-6.1-terra', reasoning: 'medium' },
  compaction: { provider: 'cpa1', model: 'gpt-6-luna', reasoning: 'medium' },
  utility: { provider: 'cpa1', model: 'gpt-6-luna', reasoning: 'low' },
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
  accessMode: 'whitelist',
  accessEntries: [
    { list: 'whitelist', scope: 'global', scopeId: null, userId: '435482941515104257' },
  ],
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
  hasUtilityModel: state.utility !== null,
})

const identityReply = (state: State): Identity => ({
  text: state.identity,
  isDefault: state.identity === DEFAULT_IDENTITY,
})

const modelsReply = (state: State): Models => ({
  main: state.main,
  compaction: state.compaction,
  utility: state.utility,
  providers: state.providers.map(provider => ({
    id: provider.id,
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
  allowed: state.accessEntries
    .filter(entry => entry.list === 'whitelist' && entry.scope === 'global')
    .map(entry => entry.userId),
  accessMode: state.accessMode,
  accessEntries: state.accessEntries,
  home: state.home,
  token: state.hasToken ? 'set' : 'missing',
  connection: state.hasToken ? { state: 'online', bot: 'Blob-1' } : { state: 'off' },
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

const checkSlot = (state: State, slot: Slot | null): Effect.Effect<Slot | null, string> =>
  slot === null || hasModel(state, slot.provider, slot.model)
    ? Effect.succeed(slot)
    : Effect.fail('Unknown model')

const synthesizeModels = (providerId: string): ReadonlyArray<ModelInfo> =>
  Array.makeBy(REFRESHED_MODEL_COUNT, index => ({
    id: `${providerId}-model-${String(index + 1).padStart(2, '0')}`,
    context: null,
    limit: null,
    images: false,
  }))

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
    if (!URL.canParse(baseUrl)) {
      return yield* Effect.fail('Base URL is not a valid URL')
    }

    return Struct.evolve(state, {
      providers: providers =>
        Array.append(providers, { id, api: payload.api, baseUrl, apiKey, models: [] }),
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
    const isInUse = [state.main, state.compaction, state.utility].some(
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

    const models = provider.models.length === 0 ? synthesizeModels(id) : provider.models

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
    const entry: DiscordAccessEntry = {
      list: 'whitelist',
      scope: 'global',
      scopeId: null,
      userId: user,
    }
    if (state.accessEntries.some(existing => sameAccessKey(existing, entry))) {
      return yield* Effect.fail('User is already allowed')
    }

    return Struct.evolve(state, {
      accessEntries: entries => sortedAccessEntries([...entries, entry]),
    })
  })

const disallowUser = (state: State, user: string): Effect.Effect<State, string> =>
  Effect.gen(function* () {
    if (!NUMERIC_ID.test(user)) {
      return yield* Effect.fail('User id must be numeric')
    }
    const entry: DiscordAccessEntry = {
      list: 'whitelist',
      scope: 'global',
      scopeId: null,
      userId: user,
    }
    if (!state.accessEntries.some(existing => sameAccessKey(existing, entry))) {
      return yield* Effect.fail('User is not allowed')
    }

    return Struct.evolve(state, {
      accessEntries: entries => entries.filter(existing => !sameAccessKey(existing, entry)),
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

const sortedAccessEntries = (
  entries: ReadonlyArray<DiscordAccessEntry>,
): ReadonlyArray<DiscordAccessEntry> =>
  entries.toSorted(
    (a, b) =>
      a.list.localeCompare(b.list) ||
      a.scope.localeCompare(b.scope) ||
      (a.scopeId ?? '').localeCompare(b.scopeId ?? '') ||
      a.userId.localeCompare(b.userId),
  )

const sameAccessKey = (
  entry: DiscordAccessEntry,
  key: {
    readonly list: DiscordAccessEntry['list']
    readonly scope: DiscordAccessEntry['scope']
    readonly scopeId: string | null
    readonly userId: string
  },
): boolean =>
  entry.list === key.list &&
  entry.scope === key.scope &&
  entry.scopeId === key.scopeId &&
  entry.userId === key.userId

const addAccessEntry = (
  state: State,
  payload: RequestPayload<'discord.addAccessEntry'>,
): Effect.Effect<State, string> =>
  Effect.gen(function* () {
    if (!SNOWFLAKE.test(payload.userId)) {
      return yield* Effect.fail('User IDs must be 17–20 digits')
    }
    if (payload.scope === 'global' && payload.scopeId !== null) {
      return yield* Effect.fail('Global access entries cannot specify a scope')
    }
    if (
      payload.scope === 'guild' &&
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
      accessEntries: entries =>
        sortedAccessEntries([...entries.filter(entry => !sameAccessKey(entry, payload)), payload]),
    })
  })

const removeAccessEntry = (
  state: State,
  payload: RequestPayload<'discord.removeAccessEntry'>,
): Effect.Effect<State, string> =>
  Effect.gen(function* () {
    if (payload.userId === '') {
      return yield* Effect.fail('Invalid user id')
    }
    if (payload.scope === 'global' && payload.scopeId !== null) {
      return yield* Effect.fail('Invalid payload')
    }
    if (payload.scope !== 'global' && (payload.scopeId === null || payload.scopeId === '')) {
      return yield* Effect.fail('Invalid scope id')
    }

    return Struct.evolve(state, {
      accessEntries: entries => entries.filter(entry => !sameAccessKey(entry, payload)),
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

type Handler<K extends RequestType> = (
  payload: RequestPayload<K>,
  state: Ref.Ref<State>,
) => Effect.Effect<ReplyPayload<K>, string>

// oxlint-disable-next-line anti-slop/no-known-value-widening -- the mapped type keeps each handler tied to its request type for generic dispatch
const handlers: { readonly [K in RequestType]: Handler<K> } = {
  'general.get': (_payload, state) => Ref.get(state).pipe(Effect.map(generalReply)),

  'general.setAutoRespond': ({ enabled }, state) =>
    transition(state, current =>
      Effect.succeed(Struct.evolve(current, { autoRespond: () => enabled })),
    ).pipe(Effect.map(generalReply)),

  'identity.get': (_payload, state) => Ref.get(state).pipe(Effect.map(identityReply)),

  'identity.set': ({ text }, state) =>
    transition(state, current =>
      text.trim() === ''
        ? Effect.fail('Identity cannot be blank')
        : Effect.succeed(Struct.evolve(current, { identity: () => text })),
    ).pipe(Effect.map(identityReply)),

  'identity.reset': (_payload, state) =>
    transition(state, current =>
      Effect.succeed(Struct.evolve(current, { identity: () => DEFAULT_IDENTITY })),
    ).pipe(Effect.map(identityReply)),

  'models.get': (_payload, state) => Ref.get(state).pipe(Effect.map(modelsReply)),

  'models.setMain': ({ provider, model, reasoning }, state) =>
    transition(state, current =>
      hasModel(current, provider, model)
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

  'discord.setAccessMode': ({ mode }, state) =>
    transition(state, current =>
      Effect.succeed(Struct.evolve(current, { accessMode: () => mode })),
    ).pipe(Effect.map(discordReply)),

  'discord.addAccessEntry': (payload, state) =>
    transition(state, current => addAccessEntry(current, payload)).pipe(Effect.map(discordReply)),

  'discord.removeAccessEntry': (payload, state) =>
    transition(state, current => removeAccessEntry(current, payload)).pipe(
      Effect.map(discordReply),
    ),

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

const dispatch = <K extends RequestType>(
  state: Ref.Ref<State>,
  frame: RequestFrame,
  type: K,
): Effect.Effect<ReplyFrame> =>
  // oxlint-disable-next-line effect-local/no-inline-schema-compile -- the payload schema depends on the request type
  Schema.decodeUnknownEffect(requests[type].payload)(frame.payload).pipe(
    Effect.mapError(() => `Invalid ${type} payload`),
    Effect.flatMap(decoded => handlers[type](decoded, state)),
    Effect.map(reply => replyOk(frame.id, reply)),
    Effect.catch(error => Effect.succeed(replyError(frame.id, error))),
  )

const serve = (state: Ref.Ref<State>, frame: RequestFrame): Effect.Effect<ReplyFrame> =>
  isRequestType(frame.type)
    ? dispatch(state, frame, frame.type)
    : Effect.succeed(replyError(frame.id, `Unknown request ${frame.type}`))

const latency = Random.nextIntBetween(150, 351).pipe(
  Effect.flatMap(ms => Effect.sleep(Duration.millis(ms))),
)

export const mockTransport: Layer.Layer<Transport> = Layer.effect(
  Transport,
  Effect.gen(function* () {
    const state = yield* Ref.make(seedState())

    return Transport.of({
      send: frame => latency.pipe(Effect.andThen(serve(state, frame))),
      status: Stream.make(true),
      unauthorized: Stream.empty,
    })
  }),
)
