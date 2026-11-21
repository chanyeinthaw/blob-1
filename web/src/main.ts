import { Effect, Match, Option, Schema } from 'effect'
import { Runtime, type Update } from 'foldkit'

import type { Api } from './api/client'
import { Message } from './message'
import { Model, SignedOut } from './model'
import * as Login from './page/login'
import { AppRoute, maybeSettingsRoute, urlToAppRoute } from './route'
import * as Session from './session'
import { RedirectToIdentity, RedirectToLogin, enterSignedIn } from './update'

export const Flags = Schema.Struct({
  isSignedIn: Schema.Boolean,
})

export type Flags = typeof Flags.Type

export const flags: Effect.Effect<Flags> = Session.isSignedIn.pipe(
  Effect.map(isSignedIn => Flags.make({ isSignedIn })),
)

type InitReturn = Update.Return<Model, Message, Api>

const initSignedOut = (route: AppRoute): InitReturn => {
  const model = SignedOut({ login: Login.init().model })

  return Match.value(route).pipe(
    Match.withReturnType<InitReturn>(),
    Match.tag('Login', () => ({ model })),
    Match.orElse(() => ({ model, commands: [RedirectToLogin()] })),
  )
}

const initSignedIn = (route: AppRoute): InitReturn =>
  Option.match(maybeSettingsRoute(route), {
    onNone: () => {
      const entry = enterSignedIn(AppRoute.Identity())

      return {
        model: entry.model,
        commands: [...entry.commands, RedirectToIdentity()],
      }
    },
    onSome: settingsRoute => {
      const entry = enterSignedIn(settingsRoute)

      return { model: entry.model, commands: entry.commands }
    },
  })

export const init: Runtime.RoutingApplicationInit<
  Model,
  Message,
  Flags,
  Api
> = (flags, url) => {
  const route = urlToAppRoute(url)

  return flags.isSignedIn ? initSignedIn(route) : initSignedOut(route)
}
