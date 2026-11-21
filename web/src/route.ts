import { Match, Option, Schema, pipe } from 'effect'
import { Route } from 'foldkit'
import { defineRouteUnion, literal, slash } from 'foldkit/route'

export const AppRoute = defineRouteUnion({
  Home: {},
  Login: {},
  Identity: {},
  Models: {},
  Providers: {},
  Discord: {},
  Mcp: {},
  NotFound: { path: Schema.String },
})

export type AppRoute = typeof AppRoute.Type

export const SettingsRoute = AppRoute.subset([
  'Identity',
  'Models',
  'Providers',
  'Discord',
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
export const discordRouter = pipe(
  settingsPath('discord'),
  Route.mapTo(AppRoute.Discord),
)
export const mcpRouter = pipe(settingsPath('mcp'), Route.mapTo(AppRoute.Mcp))

const routeParser = Route.oneOf(
  loginRouter,
  identityRouter,
  modelsRouter,
  providersRouter,
  discordRouter,
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
      'Discord',
      'Mcp',
      settingsRoute => Option.some(settingsRoute),
    ),
    Match.orElse(() => Option.none()),
  )
