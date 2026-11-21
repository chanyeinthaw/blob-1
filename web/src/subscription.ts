import { Effect, Schema, Stream } from 'effect'
import { Subscription } from 'foldkit'

import { Api } from './api/client'
import { Message } from './message'
import { Model } from './model'

const connectionStatus = Stream.unwrap(Effect.map(Api, api => api.status)).pipe(
  Stream.map(connected => Message.ReceivedApiStatus({ connected })),
)

export const subscriptions = Subscription.make<Model, Message, Api>()(
  entry => ({
    unauthorized: entry(
      { isSignedIn: Schema.Boolean },
      {
        modelToDependencies: model => ({
          isSignedIn: model._tag === 'SignedIn',
        }),
        dependenciesToStream: ({ isSignedIn }) =>
          isSignedIn
            ? Stream.unwrap(Effect.map(Api, api => api.unauthorized)).pipe(
                Stream.map(() => Message.ReceivedUnauthorized()),
              )
            : Stream.empty,
      },
    ),
    apiStatus: entry(
      { isSignedIn: Schema.Boolean },
      {
        modelToDependencies: model => ({
          isSignedIn: model._tag === 'SignedIn',
        }),
        dependenciesToStream: ({ isSignedIn }) =>
          isSignedIn ? connectionStatus : Stream.empty,
      },
    ),
  }),
)
