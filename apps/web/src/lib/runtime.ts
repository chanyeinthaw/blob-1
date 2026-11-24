import { Api, apiLayer } from '@blob-1/client/api'
import { mockTransport } from '@blob-1/client/mock'
import { sessionLive, sessionMock } from '@blob-1/client/session'
import { wsTransport } from '@blob-1/client/ws'
import type { RequestPayload, RequestType } from '@blob-1/contracts'
import { AtomMachine } from '@typeonce/effect-machine/reactivity'
import { Effect, Layer } from 'effect'
import { Atom } from 'effect/reactivity'

import { notifyLive } from './notify'

const isMock = import.meta.env.VITE_MOCK === '1'

export const sessionLayer = isMock ? sessionMock : sessionLive

/** Shared API layer memoized across the app runtime and per-bot runtimes so one socket serves all bots. */
const sharedApiLayer = apiLayer.pipe(Layer.provide(isMock ? mockTransport : wsTransport))

/** Services every page can use: the API over one shared socket, the session, and toasts. */
export const AppLayer = Layer.mergeAll(sharedApiLayer, sessionLayer, notifyLive)

export const AppRuntime = Atom.runtime(AppLayer)

export const MachineAtoms = AtomMachine.bind(AppRuntime)

const isGlobalRequest = (type: RequestType): boolean =>
  type.startsWith('providers.') ||
  type.startsWith('mcp.') ||
  type.startsWith('bots.') ||
  type.startsWith('environments.')

const scopedApiLayer = (
  botId: string,
  botGeneration: number | undefined,
): Layer.Layer<Api, never, Api> =>
  Layer.effect(
    Api,
    Effect.map(Api, api =>
      Api.of({
        ...api,
        request: <K extends RequestType>(type: K, payload: RequestPayload<K>) =>
          isGlobalRequest(type)
            ? api.request(type, payload)
            : api.request(
                type,
                Object.assign(
                  {},
                  payload,
                  botGeneration === undefined ? { botId } : { botId, botGeneration },
                ),
              ),
      }),
    ),
  )

type BoundMachineAtoms = typeof MachineAtoms
const botMachineAtomsCache = new Map<string, BoundMachineAtoms>()

/** Extracts the raw bot slug from an immutable instance key (`<botId>@<generation>`). */
export function botIdFromInstanceKey(instanceKey: string): string {
  const sep = instanceKey.indexOf('@')
  return sep === -1 ? instanceKey : instanceKey.slice(0, sep)
}

export interface BotScope {
  readonly botId: string
  readonly botGeneration?: number
}

/**
 * Resolves an instance key to the request scope. A key with no `@<generation>`
 * suffix targets whatever bot currently holds the slug.
 */
export function botScopeFromInstanceKey(instanceKey: string): BotScope {
  const sep = instanceKey.indexOf('@')
  if (sep === -1) return { botId: instanceKey }
  const generation = Number(instanceKey.slice(sep + 1))
  return Number.isFinite(generation)
    ? { botId: instanceKey.slice(0, sep), botGeneration: generation }
    : { botId: instanceKey }
}

/** Formats an immutable instance key so recreating a deleted bot ID never reuses stale state. */
export function makeBotInstanceKey(botId: string, generation: number | undefined): string {
  return generation === undefined ? botId : `${botId}@${generation}`
}

/** Returns `MachineAtoms` bound to a specific bot instance while reusing the shared WebSocket. */
export function machineAtomsForBot(scopeKey: string): BoundMachineAtoms {
  let bound = botMachineAtomsCache.get(scopeKey)
  if (bound === undefined) {
    const scope = botScopeFromInstanceKey(scopeKey)
    const botLayer = Layer.mergeAll(
      scopedApiLayer(scope.botId, scope.botGeneration).pipe(Layer.provide(sharedApiLayer)),
      sessionLayer,
      notifyLive,
    )
    bound = AtomMachine.bind(Atom.runtime(botLayer))
    botMachineAtomsCache.set(scopeKey, bound)
  }
  return bound
}
