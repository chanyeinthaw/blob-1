import { Context, Effect, Layer, Ref, Schema, Stream } from 'effect'

import {
  type ChangeFrame,
  type ReplyFrame,
  type ReplyPayload,
  type RequestFrame,
  type RequestPayload,
  type RequestType,
  requests,
} from '@blob-1/contracts'

export class ApiError extends Schema.TaggedError<ApiError>()('ApiError', {
  message: Schema.String,
}) {}

/**
 * A server `changed` push, or `resync` after the socket reconnects: pushes
 * missed while offline are lost, so everything shown may be stale.
 */
export type DataChange = ChangeFrame | { readonly type: 'resync' }

export type ApiClient = Readonly<{
  request: <K extends RequestType>(
    type: K,
    payload: RequestPayload<K>,
  ) => Effect.Effect<ReplyPayload<K>, ApiError>
  status: Stream.Stream<boolean>
  unauthorized: Stream.Stream<boolean>
  /** Server pushes that a bot's data changed; refetch what the topic covers. */
  changes: Stream.Stream<DataChange>
}>

export class Api extends Context.Service<Api, ApiClient>()('@blob-1/client/api') {}

export type TransportService = Readonly<{
  send: (frame: RequestFrame) => Effect.Effect<ReplyFrame, ApiError>
  status: Stream.Stream<boolean>
  unauthorized: Stream.Stream<boolean>
  changes: Stream.Stream<DataChange>
}>

export class Transport extends Context.Service<Transport, TransportService>()(
  '@blob-1/client/api/Transport',
) {}

const decodeReply = <K extends RequestType>(
  type: K,
  reply: ReplyFrame,
): Effect.Effect<ReplyPayload<K>, ApiError> =>
  reply.ok
    ? // oxlint-disable-next-line effect-local/no-inline-schema-compile -- the reply schema depends on the request type
      Schema.decodeUnknownEffect(requests[type].reply)(reply.payload).pipe(
        Effect.mapError(() => new ApiError({ message: `Unexpected reply to ${type}` })),
      )
    : Effect.fail(new ApiError({ message: reply.error }))

const makeApi = Effect.gen(function* () {
  const transport = yield* Transport
  const sequence = yield* Ref.make(0)

  const request = <K extends RequestType>(
    type: K,
    payload: RequestPayload<K>,
  ): Effect.Effect<ReplyPayload<K>, ApiError> =>
    Ref.updateAndGet(sequence, n => n + 1).pipe(
      Effect.flatMap(id => transport.send({ id: String(id), type, payload })),
      Effect.flatMap(reply => decodeReply(type, reply)),
    )

  return Api.of({
    request,
    status: transport.status,
    unauthorized: transport.unauthorized,
    changes: transport.changes,
  })
})

export const apiLayer: Layer.Layer<Api, never, Transport> = Layer.effect(Api, makeApi)

export const request = <K extends RequestType>(type: K, payload: RequestPayload<K>) =>
  Effect.flatMap(Api, api => api.request(type, payload))
