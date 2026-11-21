import { Effect, Match, Option, Schema } from 'effect'
import { Command, Update } from 'foldkit'
import { UrlRequest, load, pushUrl, replaceUrl } from 'foldkit/navigation'
import { modifyFields } from 'foldkit/struct'
import { toString as urlToString } from 'foldkit/url'

import type { Api } from './api/client'
import { Message } from './message'
import { Connection, Model, Page, SignedIn, SignedOut } from './model'
import * as Login from './page/login'
import * as Discord from './page/settings/discord'
import * as Identity from './page/settings/identity'
import * as Mcp from './page/settings/mcp'
import * as Models from './page/settings/models'
import * as Providers from './page/settings/providers'
import {
  AppRoute,
  SettingsRoute,
  identityRouter,
  loginRouter,
  maybeSettingsRoute,
  urlToAppRoute,
} from './route'
import * as Session from './session'
import * as Toast from './ui/toast'

const NavigateInternal = Command.define('NavigateInternal', {
  args: { url: Schema.String },
  messages: [Message.CompletedNavigateInternal],
  execute: ({ url }) =>
    pushUrl(url).pipe(Effect.as(Message.CompletedNavigateInternal())),
})

const LoadExternal = Command.define('LoadExternal', {
  args: { href: Schema.String },
  messages: [Message.CompletedLoadExternal],
  execute: ({ href }) =>
    load(href).pipe(Effect.as(Message.CompletedLoadExternal())),
})

export const RedirectToLogin = Command.define('RedirectToLogin', {
  messages: [Message.CompletedNavigateInternal],
  execute: replaceUrl(loginRouter()).pipe(
    Effect.as(Message.CompletedNavigateInternal()),
  ),
})

export const RedirectToIdentity = Command.define('RedirectToIdentity', {
  messages: [Message.CompletedNavigateInternal],
  execute: replaceUrl(identityRouter()).pipe(
    Effect.as(Message.CompletedNavigateInternal()),
  ),
})

const SignOut = Command.define('SignOut', {
  messages: [Message.CompletedSignOut, Message.FailedSignOut],
  execute: Session.signOut.pipe(
    Effect.as(Message.CompletedSignOut()),
    Effect.catch(() => Effect.succeed(Message.FailedSignOut())),
  ),
})

type UpdateReturn = Update.Return<Model, Message, Api>
const withUpdateReturn = Match.withReturnType<UpdateReturn>()

type PageEntry = Readonly<{
  page: Page
  commands: Update.Commands<Message, Api>
}>

type SignedInEntry = Readonly<{
  model: SignedIn
  commands: Update.Commands<Message, Api>
}>

const liftPageInit = <PageModel, PageMessage>(
  init: Update.Return<PageModel, PageMessage, Api>,
  toPage: (model: PageModel) => Page,
  toMessage: (message: PageMessage) => Message,
): PageEntry => ({
  page: toPage(init.model),
  commands: Command.mapMessages(init.commands, toMessage),
})

const enterPage = (route: SettingsRoute): PageEntry =>
  Match.value(route).pipe(
    Match.tagsExhaustive({
      Identity: () =>
        liftPageInit(
          Identity.init(),
          model => Page.Identity({ model }),
          message => Message.GotIdentityMessage({ message }),
        ),
      Models: () =>
        liftPageInit(
          Models.init(),
          model => Page.Models({ model }),
          message => Message.GotModelsMessage({ message }),
        ),
      Providers: () =>
        liftPageInit(
          Providers.init(),
          model => Page.Providers({ model }),
          message => Message.GotProvidersMessage({ message }),
        ),
      Discord: () =>
        liftPageInit(
          Discord.init(),
          model => Page.Discord({ model }),
          message => Message.GotDiscordMessage({ message }),
        ),
      Mcp: () =>
        liftPageInit(
          Mcp.init(),
          model => Page.Mcp({ model }),
          message => Message.GotMcpMessage({ message }),
        ),
    }),
  )

export const enterSignedIn = (route: SettingsRoute): SignedInEntry => {
  const entry = enterPage(route)

  return {
    model: SignedIn({
      connection: Connection.Disconnected(),
      toast: Toast.init({ id: 'toast' }),
      page: entry.page,
    }),
    commands: entry.commands,
  }
}

const readSignedIn = (model: Model): Option.Option<SignedIn> =>
  Match.value(model).pipe(
    Match.tag('SignedIn', signedIn => Option.some(signedIn)),
    Match.orElse(() => Option.none()),
  )

const writePage = (model: Model, page: Page): Model =>
  Match.value(model).pipe(
    Match.tag('SignedIn', signedIn =>
      modifyFields(signedIn, { page: () => page }),
    ),
    Match.orElse(() => model),
  )

const writeToast = (model: Model, toast: typeof Toast.Model.Type): Model =>
  Match.value(model).pipe(
    Match.tag('SignedIn', signedIn =>
      modifyFields(signedIn, { toast: () => toast }),
    ),
    Match.orElse(() => model),
  )

const readToast = (model: Model): Option.Option<typeof Toast.Model.Type> =>
  Option.map(readSignedIn(model), ({ toast }) => toast)

const foldToastOutMessage = Toast.OutMessage.match<
  Update.Step<Model, Message, Api>
>({
  DismissedToast: () => model => ({ model }),
})

const foldToast = Update.foldChild({
  update: Toast.update,
  read: readToast,
  write: writeToast,
  toParentMessage: message => Message.GotToastMessage({ message }),
  foldOutMessage: foldToastOutMessage,
})

const foldShowToast = Update.foldChild({
  update: Toast.show,
  read: readToast,
  write: writeToast,
  toParentMessage: message => Message.GotToastMessage({ message }),
  foldOutMessage: foldToastOutMessage,
})

const foldShowedToast =
  ({
    variant,
    title,
  }: Readonly<{ variant: Toast.ToastVariant; title: string }>): Update.Step<
    Model,
    Message,
    Api
  > =>
  model =>
    foldShowToast(model, { variant, payload: { title } })

const foldLogin = Update.foldChild({
  update: Login.update,
  read: (model: Model) =>
    Match.value(model).pipe(
      Match.tag('SignedOut', ({ login }) => Option.some(login)),
      Match.orElse(() => Option.none()),
    ),
  write: (_model: Model, nextLogin: Login.Model): Model =>
    SignedOut({ login: nextLogin }),
  toParentMessage: message => Message.GotLoginMessage({ message }),
  foldOutMessage: Login.OutMessage.match<Update.Step<Model, Message, Api>>({
    SignedIn: () => () => {
      const entry = enterSignedIn(AppRoute.Identity())

      return {
        model: entry.model,
        commands: [...entry.commands, RedirectToIdentity()],
      }
    },
  }),
})

const readIdentityPage = (page: Page): Option.Option<Identity.Model> =>
  Match.value(page).pipe(
    Match.tag('Identity', ({ model }) => Option.some(model)),
    Match.orElse(() => Option.none()),
  )

const foldIdentity = Update.foldChild({
  update: Identity.update,
  read: (model: Model) =>
    Option.flatMap(readSignedIn(model), ({ page }) => readIdentityPage(page)),
  write: (model: Model, nextIdentity: Identity.Model) =>
    writePage(model, Page.Identity({ model: nextIdentity })),
  toParentMessage: message => Message.GotIdentityMessage({ message }),
  foldOutMessage: Identity.OutMessage.match<Update.Step<Model, Message, Api>>({
    ShowedToast: foldShowedToast,
  }),
})

const readModelsPage = (page: Page): Option.Option<Models.Model> =>
  Match.value(page).pipe(
    Match.tag('Models', ({ model }) => Option.some(model)),
    Match.orElse(() => Option.none()),
  )

const foldModels = Update.foldChild({
  update: Models.update,
  read: (model: Model) =>
    Option.flatMap(readSignedIn(model), ({ page }) => readModelsPage(page)),
  write: (model: Model, nextModels: Models.Model) =>
    writePage(model, Page.Models({ model: nextModels })),
  toParentMessage: message => Message.GotModelsMessage({ message }),
  foldOutMessage: Models.OutMessage.match<Update.Step<Model, Message, Api>>({
    ShowedToast: foldShowedToast,
  }),
})

const readProvidersPage = (page: Page): Option.Option<Providers.Model> =>
  Match.value(page).pipe(
    Match.tag('Providers', ({ model }) => Option.some(model)),
    Match.orElse(() => Option.none()),
  )

const foldProviders = Update.foldChild({
  update: Providers.update,
  read: (model: Model) =>
    Option.flatMap(readSignedIn(model), ({ page }) => readProvidersPage(page)),
  write: (model: Model, nextProviders: Providers.Model) =>
    writePage(model, Page.Providers({ model: nextProviders })),
  toParentMessage: message => Message.GotProvidersMessage({ message }),
  foldOutMessage: Providers.OutMessage.match<Update.Step<Model, Message, Api>>({
    ShowedToast: foldShowedToast,
  }),
})

const readDiscordPage = (page: Page): Option.Option<Discord.Model> =>
  Match.value(page).pipe(
    Match.tag('Discord', ({ model }) => Option.some(model)),
    Match.orElse(() => Option.none()),
  )

const foldDiscord = Update.foldChild({
  update: Discord.update,
  read: (model: Model) =>
    Option.flatMap(readSignedIn(model), ({ page }) => readDiscordPage(page)),
  write: (model: Model, nextDiscord: Discord.Model) =>
    writePage(model, Page.Discord({ model: nextDiscord })),
  toParentMessage: message => Message.GotDiscordMessage({ message }),
  foldOutMessage: Discord.OutMessage.match<Update.Step<Model, Message, Api>>({
    ShowedToast: foldShowedToast,
  }),
})

const readMcpPage = (page: Page): Option.Option<Mcp.Model> =>
  Match.value(page).pipe(
    Match.tag('Mcp', ({ model }) => Option.some(model)),
    Match.orElse(() => Option.none()),
  )

const foldMcp = Update.foldChild({
  update: Mcp.update,
  read: (model: Model) =>
    Option.flatMap(readSignedIn(model), ({ page }) => readMcpPage(page)),
  write: (model: Model, nextMcp: Mcp.Model) =>
    writePage(model, Page.Mcp({ model: nextMcp })),
  toParentMessage: message => Message.GotMcpMessage({ message }),
  foldOutMessage: Mcp.OutMessage.match<Update.Step<Model, Message, Api>>({
    ShowedToast: foldShowedToast,
    RequestedLogin:
      ({ url }) =>
      model => ({ model, commands: [LoadExternal({ href: url })] }),
  }),
})

const signOut = (model: Model): UpdateReturn =>
  Match.value(model).pipe(
    withUpdateReturn,
    Match.tag('SignedIn', () => ({
      model,
      commands: [SignOut()],
    })),
    Match.orElse(() => ({ model })),
  )

const receivedApiStatus = (model: Model, connected: boolean): UpdateReturn =>
  Match.value(model).pipe(
    withUpdateReturn,
    Match.tag('SignedIn', signedIn => ({
      model: modifyFields(signedIn, {
        connection: () =>
          connected ? Connection.Connected() : Connection.Disconnected(),
      }),
    })),
    Match.orElse(() => ({ model })),
  )

const showPage = (
  model: Model,
  signedIn: SignedIn,
  route: SettingsRoute,
): UpdateReturn => {
  if (signedIn.page._tag === route._tag) {
    return { model }
  }

  const entry = enterPage(route)

  return {
    model: modifyFields(signedIn, { page: () => entry.page }),
    commands: entry.commands,
  }
}

const routeChanged = (model: Model, route: AppRoute): UpdateReturn =>
  Match.value(model).pipe(
    withUpdateReturn,
    Match.tagsExhaustive({
      SignedOut: () =>
        Match.value(route).pipe(
          withUpdateReturn,
          Match.tag('Login', () => ({ model })),
          Match.orElse(() => ({ model, commands: [RedirectToLogin()] })),
        ),
      SignedIn: signedIn =>
        Option.match(maybeSettingsRoute(route), {
          onNone: () => ({ model, commands: [RedirectToIdentity()] }),
          onSome: settingsRoute => showPage(model, signedIn, settingsRoute),
        }),
    }),
  )

export const update = (model: Model, message: Message) =>
  Message.match<UpdateReturn>(message, {
    ClickedLink: ({ request }) =>
      UrlRequest.match<UpdateReturn>(request, {
        Internal: ({ url }) => ({
          model,
          commands: [NavigateInternal({ url: urlToString(url) })],
        }),
        External: ({ href }) => ({
          model,
          commands: [LoadExternal({ href })],
        }),
      }),

    ChangedUrl: ({ url }) => routeChanged(model, urlToAppRoute(url)),

    ClickedSignOut: () => signOut(model),

    ReceivedApiStatus: ({ connected }) => receivedApiStatus(model, connected),

    ReceivedUnauthorized: () =>
      model._tag === 'SignedIn'
        ? {
            model: SignedOut({ login: Login.init().model }),
            commands: [RedirectToLogin()],
          }
        : { model },

    GotLoginMessage: ({ message }) => foldLogin(model, message),
    GotIdentityMessage: ({ message }) => foldIdentity(model, message),
    GotModelsMessage: ({ message }) => foldModels(model, message),
    GotProvidersMessage: ({ message }) => foldProviders(model, message),
    GotDiscordMessage: ({ message }) => foldDiscord(model, message),
    GotMcpMessage: ({ message }) => foldMcp(model, message),
    GotToastMessage: ({ message }) => foldToast(model, message),

    CompletedNavigateInternal: () => ({ model }),
    CompletedLoadExternal: () => ({ model }),
    CompletedSignOut: () =>
      model._tag === 'SignedIn'
        ? {
            model: SignedOut({ login: Login.init().model }),
            commands: [RedirectToLogin()],
          }
        : { model },
    FailedSignOut: () =>
      foldShowedToast({
        variant: 'Error',
        title: 'Sign out failed. Try again.',
      })(model),
  })
