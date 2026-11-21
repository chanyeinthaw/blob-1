import { clsx } from 'clsx'
import { Html, HtmlBuilder } from 'foldkit/html'

import { Message } from './message'
import { Connection, Page, SignedIn } from './model'
import * as Discord from './page/settings/discord'
import * as Identity from './page/settings/identity'
import * as Models from './page/settings/models'
import * as Providers from './page/settings/providers'
import {
  discordRouter,
  identityRouter,
  modelsRouter,
  providersRouter,
} from './route'
import * as Blob from './ui/blob'
import * as Icon from './ui/icon'
import * as Toast from './ui/toast'

const isAwake = (connection: Connection): boolean =>
  Connection.match(connection, {
    Connected: () => true,
    Disconnected: () => false,
  })

const statusView = (connection: Connection, h: HtmlBuilder<Message>): Html =>
  h.p(
    [
      h.Class(
        clsx(
          'flex items-center gap-1.5 text-caption leading-tight',
          isAwake(connection) ? 'text-mute' : 'text-alarm',
        ),
      ),
    ],
    [
      h.span([
        h.AriaHidden(true),
        h.Class(
          clsx(
            'size-1.5 rounded-full',
            isAwake(connection) ? 'bg-ok' : 'bg-alarm',
          ),
        ),
      ]),
      isAwake(connection) ? 'Connected' : 'Offline',
    ],
  )

type NavItem = Readonly<{
  label: string
  href: string
  isActive: boolean
  icon: (className: string) => Html
}>

const navItems = (page: Page): ReadonlyArray<NavItem> => [
  {
    label: 'Identity',
    href: identityRouter(),
    isActive: page._tag === 'Identity',
    icon: Icon.user,
  },
  {
    label: 'Models',
    href: modelsRouter(),
    isActive: page._tag === 'Models',
    icon: Icon.sliders,
  },
  {
    label: 'Providers',
    href: providersRouter(),
    isActive: page._tag === 'Providers',
    icon: Icon.server,
  },
  {
    label: 'Discord',
    href: discordRouter(),
    isActive: page._tag === 'Discord',
    icon: Icon.chat,
  },
]

const focusClassName =
  'focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring'

const sidebarLinkView = (item: NavItem, h: HtmlBuilder<Message>): Html =>
  h.a(
    [
      h.Href(item.href),
      ...(item.isActive ? [h.AriaCurrent('page')] : []),
      h.Class(
        clsx(
          'flex h-8 items-center gap-2 rounded-md px-2 text-body',
          focusClassName,
          item.isActive
            ? 'bg-hover font-medium text-ink'
            : 'text-slate hover:bg-hover hover:text-ink',
        ),
      ),
    ],
    [item.icon('size-4 shrink-0'), item.label],
  )

const sidebarView = (
  connection: Connection,
  page: Page,
  h: HtmlBuilder<Message>,
): Html =>
  h.header(
    [h.Class('fixed inset-y-0 left-0 hidden w-56 flex-col p-2 md:flex')],
    [
      h.div(
        [h.Class('mb-2 flex items-center gap-2 p-2')],
        [
          h.span(
            [
              h.Class(
                'grid size-8 shrink-0 place-items-center rounded-lg bg-accent text-white',
              ),
            ],
            [Blob.view('size-4', h)],
          ),
          h.div(
            [h.Class('min-w-0')],
            [
              h.p(
                [h.Class('text-body font-semibold leading-tight text-ink')],
                ['Blob-1'],
              ),
              statusView(connection, h),
            ],
          ),
        ],
      ),
      h.p(
        [
          h.Class(
            'flex h-8 items-center px-2 text-caption font-medium text-mute',
          ),
        ],
        ['Settings'],
      ),
      h.nav(
        [h.Class('flex flex-col gap-1'), h.AriaLabel('Settings')],
        navItems(page).map(item => sidebarLinkView(item, h)),
      ),
      h.button(
        [
          h.Type('button'),
          h.OnClick(Message.ClickedSignOut()),
          h.Class(
            clsx(
              'mt-auto flex h-8 cursor-pointer items-center gap-2 rounded-md px-2 text-body text-slate hover:bg-hover hover:text-ink',
              focusClassName,
            ),
          ),
        ],
        [Icon.signOut('size-4 shrink-0'), 'Sign out'],
      ),
    ],
  )

const tabClassName = (isActive: boolean): string =>
  clsx(
    'flex cursor-pointer flex-col items-center justify-center gap-0.5 rounded-lg text-[11px] font-medium',
    focusClassName,
    isActive ? 'text-ink' : 'text-mute',
  )

const tabBarView = (page: Page, h: HtmlBuilder<Message>): Html =>
  h.nav(
    [
      h.Class(
        'fixed inset-x-0 bottom-0 z-20 grid h-16 grid-cols-5 bg-shelf px-1.5 pb-2 pt-1.5 md:hidden',
      ),
      h.AriaLabel('Settings'),
    ],
    [
      ...navItems(page).map(item =>
        h.a(
          [
            h.Href(item.href),
            ...(item.isActive ? [h.AriaCurrent('page')] : []),
            h.Class(tabClassName(item.isActive)),
          ],
          [item.icon('size-5'), item.label],
        ),
      ),
      h.button(
        [
          h.Type('button'),
          h.OnClick(Message.ClickedSignOut()),
          h.Class(tabClassName(false)),
        ],
        [Icon.signOut('size-5'), 'Sign out'],
      ),
    ],
  )

const pageView = (page: Page, h: HtmlBuilder<Message>): Html =>
  Page.match(page, {
    Identity: ({ model }) =>
      h.submodel({
        slotId: 'identity',
        model,
        view: Identity.view,
        toParentMessage: message => Message.GotIdentityMessage({ message }),
      }),
    Models: ({ model }) =>
      h.submodel({
        slotId: 'models',
        model,
        view: Models.view,
        toParentMessage: message => Message.GotModelsMessage({ message }),
      }),
    Providers: ({ model }) =>
      h.submodel({
        slotId: 'providers',
        model,
        view: Providers.view,
        toParentMessage: message => Message.GotProvidersMessage({ message }),
      }),
    Discord: ({ model }) =>
      h.submodel({
        slotId: 'discord',
        model,
        view: Discord.view,
        toParentMessage: message => Message.GotDiscordMessage({ message }),
      }),
  })

export const shellView = (signedIn: SignedIn, h: HtmlBuilder<Message>): Html =>
  h.div(
    [h.Class('min-h-dvh bg-shelf')],
    [
      sidebarView(signedIn.connection, signedIn.page, h),
      h.main(
        [h.Class('min-w-0 pb-16 md:ml-56 md:h-dvh md:p-2 md:pb-2 md:pl-0')],
        [pageView(signedIn.page, h)],
      ),
      tabBarView(signedIn.page, h),
      Toast.view(
        signedIn.toast,
        message => Message.GotToastMessage({ message }),
        h,
      ),
    ],
  )
