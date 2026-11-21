import { Match, Option, Schema, pipe } from 'effect'
import { Route } from 'foldkit'
import { defineRouteUnion, literal, slash } from 'foldkit/route'

export const AppRoute = defineRouteUnion({
  Home: {},
  Login: {},
  Identity: {},
  Models: {},
  Providers: {},
  Chat: {},
  ChatDiscord: {},
  Mcp: {},
  NotFound: { path: Schema.String },
})

export type AppRoute = typeof AppRoute.Type

export const SettingsRoute = AppRoute.subset([
  'Identity',
  'Models',
  'Providers',
  'Chat',
  'ChatDiscord',
  'Mcp',
])

export type SettingsRoute = typeof SettingsRoute.Type

export const homeRouter = pipe(Route.root, Route.mapTo(AppRoute.Home))
export const loginRouter = pipe(literal('login'), Route.mapTo(AppRoute.Login))

const settingsPath = (page: string) =>
  pipe(literal('settings'), slash(literal(page)))

export const identityRouter = pipe(
  settingsPath('identity'),
  Route.mapTo(AppRoute.Identity),
)
export const modelsRouter = pipe(
  settingsPath('models'),
  Route.mapTo(AppRoute.Models),
)
export const providersRouter = pipe(
  settingsPath('providers'),
  Route.mapTo(AppRoute.Providers),
)
export const chatRouter = pipe(settingsPath('chat'), Route.mapTo(AppRoute.Chat))
export const chatDiscordRouter = pipe(
  settingsPath('chat'),
  slash(literal('discord')),
  Route.mapTo(AppRoute.ChatDiscord),
)
const legacyDiscordRouter = pipe(
  settingsPath('discord'),
  Route.mapTo(AppRoute.ChatDiscord),
)
export const mcpRouter = pipe(settingsPath('mcp'), Route.mapTo(AppRoute.Mcp))

const routeParser = Route.oneOf(
  loginRouter,
  identityRouter,
  modelsRouter,
  providersRouter,
  chatDiscordRouter,
  chatRouter,
  legacyDiscordRouter,
  mcpRouter,
  homeRouter,
)

export const urlToAppRoute = Route.parseUrlWithFallback(
  routeParser,
  AppRoute.NotFound,
)

export const maybeSettingsRoute = (
  route: AppRoute,
): Option.Option<SettingsRoute> =>
  Match.value(route).pipe(
    Match.tag(
      'Identity',
      'Models',
      'Providers',
      'Chat',
      'ChatDiscord',
      'Mcp',
      settingsRoute => Option.some(settingsRoute),
    ),
    Match.orElse(() => Option.none()),
  )
