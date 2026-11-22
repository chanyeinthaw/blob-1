import { Array, Duration, Effect, Layer, Random, Ref, Schema, Stream, Struct } from 'effect'

import { Transport } from './api.ts'
import {
  Discord,
  type DiscordInteractionOverride,
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
  Slot,
  isRequestType,
  requests,
} from '@blob-1/contracts'

type ProviderRecord = Readonly<{
  id: string
  api: ProviderApi
  baseUrl: string
  keyEnv: string
  models: ReadonlyArray<ModelInfo>
}>

type State = Readonly<{
  identity: string
  main: Slot
  compaction: Slot | null
  utility: Slot | null
  providers: ReadonlyArray<ProviderRecord>
  allowed: ReadonlyArray<string>
  home: Home
  hasToken: boolean
  interactionModes: ReadonlyArray<DiscordInteractionOverride>
  mcpServers: ReadonlyArray<McpServer>
}>

const DEFAULT_IDENTITY = 'You are Blob-1.'
const REFRESHED_MODEL_COUNT = 49
const NUMERIC_ID = /^\d+$/

const seedState = (): State => ({
  identity: DEFAULT_IDENTITY,
  main: { provider: 'cpa1', model: 'gpt-6.1-terra', reasoning: 'medium' },
  compaction: { provider: 'cpa1', model: 'gpt-6-luna', reasoning: 'medium' },
  utility: { provider: 'cpa1', model: 'gpt-6-luna', reasoning: 'low' },
  providers: [
    {
      id: 'cpa1',
      api: 'responses',
      baseUrl: 'http://127.0.0.1:8320/v1',
      keyEnv: 'CPA1_KEY',
      models: [
        { id: 'gpt-6.1-terra', context: 1050000, limit: 400000, images: true },
        { id: 'gpt-6-luna', context: 1050000, limit: null, images: true },
        { id: 'gpt-6-luna-fast', context: null, limit: null, images: false },
        { id: 'claude-fable-5', context: 1000000, limit: null, images: false },
        { id: 'gpt-5.6-terra', context: 1050000, limit: null, images: false },
      ],
    },
  ],
  allowed: ['435482941515104257'],
  home: { kind: 'none' },
  hasToken: true,
  interactionModes: [],
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
    keyEnv: provider.keyEnv,
    modelCount: provider.models.length,
  }))

const discordReply = (state: State): Discord => ({
  allowed: state.allowed,
  home: state.home,
  token: state.hasToken ? 'set' : 'missing',
  connection: state.hasToken ? { state: 'online', bot: 'Blob-1' } : { state: 'off' },
  interactionModes: state.interactionModes,
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
    const { id, api, baseUrl, keyEnv } = payload

    if ([id, baseUrl, keyEnv].some(value => value.trim() === '')) {
      return yield* Effect.fail('All fields are required')
    }
    if (state.providers.some(provider => provider.id === id)) {
      return yield* Effect.fail(`Provider ${id} already exists`)
    }
    if (!URL.canParse(baseUrl)) {
      return yield* Effect.fail('Base URL is not a valid URL')
    }

    return Struct.evolve(state, {
      providers: providers => Array.append(providers, { id, api, baseUrl, keyEnv, models: [] }),
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
    if (state.allowed.includes(user)) {
      return yield* Effect.fail('User is already allowed')
    }

    return Struct.evolve(state, {
      allowed: allowed => Array.append(allowed, user),
    })
  })

const disallowUser = (state: State, user: string): Effect.Effect<State, string> =>
  Effect.gen(function* () {
    if (!NUMERIC_ID.test(user)) {
      return yield* Effect.fail('User id must be numeric')
    }
    if (!state.allowed.includes(user)) {
      return yield* Effect.fail('User is not allowed')
    }

    return Struct.evolve(state, {
      allowed: allowed => Array.filter(allowed, entry => entry !== user),
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
  modes.toSorted((a, b) => a.scope.localeCompare(b.scope) || a.targetId.localeCompare(b.targetId))

const setInteractionMode = (
  state: State,
  payload: RequestPayload<'discord.setInteractionMode'>,
): Effect.Effect<State, string> =>
  Effect.gen(function* () {
    if (!SNOWFLAKE.test(payload.targetId)) {
      return yield* Effect.fail('Ids must be 17–20 digits')
    }

    return Struct.evolve(state, {
      interactionModes: modes =>
        sortedModes([
          ...modes.filter(
            entry => !(entry.scope === payload.scope && entry.targetId === payload.targetId),
          ),
          payload,
        ]),
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

    return Struct.evolve(state, {
      interactionModes: modes =>
        modes.filter(
          entry => !(entry.scope === payload.scope && entry.targetId === payload.targetId),
        ),
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

  'providers.remove': ({ id }, state) =>
    transition(state, current => removeProvider(current, id)).pipe(Effect.map(providersReply)),

  'providers.refresh': ({ id }, state) =>
    transition(state, current => refreshProvider(current, id)).pipe(Effect.map(providersReply)),

  'discord.get': (_payload, state) => Ref.get(state).pipe(Effect.map(discordReply)),

  'discord.directory': () =>
    Effect.succeed({
      channels: [
        { id: '928368271290421338', name: 'server-man', server: 'Blob Lab' },
        { id: '928368271290421339', name: 'general', server: 'Blob Lab' },
        { id: '928368271290421340', name: 'eng', server: 'Blob Lab' },
        { id: '842724454677348360', name: 'announcements', server: 'Studio' },
      ],
      people: [
        { id: '435482941515104257', name: 'Chan Nyein Thaw' },
        { id: '694730852973805679', name: 'Su Hlaing Hnin' },
        { id: '1090496556781809745', name: 'Pinn' },
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
