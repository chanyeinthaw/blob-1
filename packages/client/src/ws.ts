import { Effect, Layer, PubSub, Schema, Stream, SubscriptionRef } from 'effect'

import { ApiError, type DataChange, Transport } from './api.ts'
import { ChangeFrame, ReplyFrame, type RequestFrame } from '@blob-1/contracts'

/** Everything the server sends on `/ws`: request replies and `changed` pushes. */
const decodeServerFrame = Schema.decodeUnknownSync(Schema.Union([ChangeFrame, ReplyFrame]))

type Pending = Readonly<{
  frame: RequestFrame
  resume: (result: Effect.Effect<ReplyFrame, ApiError>) => void
}>

export const wsTransport: Layer.Layer<Transport> = Layer.effect(
  Transport,
  Effect.gen(function* () {
    const connected = yield* SubscriptionRef.make(false)
    const unauthorized = yield* SubscriptionRef.make(false)
    const changes = yield* PubSub.unbounded<DataChange>()
    const pending = new Map<string, Pending>()
    let socket: WebSocket | undefined
    let reconnectTimer: ReturnType<typeof setTimeout> | undefined
    let attempts = 0
    let stopped = true
    let everOpened = false

    const setConnected = (value: boolean) => Effect.runSync(SubscriptionRef.set(connected, value))
    const setUnauthorized = (value: boolean) =>
      Effect.runSync(SubscriptionRef.set(unauthorized, value))

    const sendFrame = (frame: RequestFrame) => {
      if (socket?.readyState === WebSocket.OPEN) {
        socket.send(JSON.stringify(frame))
      }
    }

    const connect = () => {
      const protocol = location.protocol === 'https:' ? 'wss:' : 'ws:'
      const current = new WebSocket(`${protocol}//${location.host}/ws?changes=1`)
      socket = current
      let opened = false

      current.addEventListener('open', () => {
        if (stopped || socket !== current) {
          current.close()
          return
        }
        opened = true
        if (everOpened) {
          PubSub.publishUnsafe(changes, { type: 'resync' })
        }
        everOpened = true
        attempts = 0
        setUnauthorized(false)
        setConnected(true)
        for (const { frame } of pending.values()) {
          sendFrame(frame)
        }
      })
      current.addEventListener('message', event => {
        try {
          const frame = decodeServerFrame(JSON.parse(String(event.data)))
          if ('topic' in frame) {
            PubSub.publishUnsafe(changes, frame)
            return
          }
          const reply = frame
          const request = pending.get(reply.id)
          if (request !== undefined) {
            pending.delete(reply.id)
            request.resume(Effect.succeed(reply))
          }
        } catch {
          current.close()
        }
      })
      current.addEventListener('close', event => {
        if (socket !== current) {
          return
        }
        setConnected(false)
        if (opened) {
          for (const request of pending.values()) {
            request.resume(Effect.fail(new ApiError({ message: 'Connection lost' })))
          }
          pending.clear()
        }
        if (event.code === 4401) {
          setUnauthorized(true)
          return
        }
        void fetch('/api/session')
          .then(response => {
            if (!stopped && socket === current && response.status === 401) {
              setUnauthorized(true)
            }
          })
          .catch(() => {})
        if (!stopped) {
          const delay = Math.min(1000 * 2 ** attempts, 30000)
          attempts = Math.min(attempts + 1, 5)
          reconnectTimer = setTimeout(connect, delay)
        }
      })
    }

    const stop = () => {
      stopped = true
      clearTimeout(reconnectTimer)
      const current = socket
      socket = undefined
      current?.close()
      setConnected(false)
      setUnauthorized(false)
      for (const request of pending.values()) {
        request.resume(Effect.fail(new ApiError({ message: 'Connection closed' })))
      }
      pending.clear()
    }

    yield* Effect.addFinalizer(() => Effect.sync(stop))

    return Transport.of({
      send: frame =>
        Effect.callback<ReplyFrame, ApiError>(resume => {
          const timer = setTimeout(() => {
            pending.delete(frame.id)
            resume(Effect.fail(new ApiError({ message: 'Request timed out' })))
          }, 15000)
          pending.set(frame.id, {
            frame,
            resume: result => {
              clearTimeout(timer)
              resume(result)
            },
          })
          sendFrame(frame)
          return Effect.sync(() => {
            clearTimeout(timer)
            pending.delete(frame.id)
          })
        }),
      status: Stream.unwrap(
        Effect.acquireRelease(
          Effect.sync(() => {
            stopped = false
            attempts = 0
            setUnauthorized(false)
            connect()
            return SubscriptionRef.changes(connected)
          }),
          () => Effect.sync(stop),
        ),
      ),
      unauthorized: SubscriptionRef.changes(unauthorized).pipe(Stream.filter(Boolean)),
      changes: Stream.fromPubSub(changes),
    })
  }),
)
