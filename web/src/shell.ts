import { clsx } from 'clsx'
import { Html, HtmlBuilder } from 'foldkit/html'

import { Message } from './message'
import { Connection, Page, SignedIn } from './model'
import * as Chat from './page/settings/chat'
import * as Identity from './page/settings/identity'
import * as Mcp from './page/settings/mcp'
import * as Models from './page/settings/models'
import * as Providers from './page/settings/providers'
import {
  chatRouter,
  identityRouter,
  mcpRouter,
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

const statusDotView = (
  connection: Connection,
  h: HtmlBuilder<Message>,
): Html => {
  const label = isAwake(connection) ? 'Connected' : 'Offline'
  return h.span(
    [h.Title(label), h.Class('flex shrink-0 items-center')],
    [
      h.span([
        h.AriaHidden(true),
        h.Class(
          clsx(
            'size-2 rounded-full',
            isAwake(connection) ? 'bg-ok' : 'bg-alarm',
          ),
        ),
      ]),
      h.span([h.Class('sr-only')], [label]),
    ],
  )
}

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
    label: 'Chat platforms',
    href: chatRouter(),
    isActive: page._tag === 'Chat',
    icon: Icon.chat,
  },
  {
    label: 'MCP',
    href: mcpRouter(),
    isActive: page._tag === 'Mcp',
    icon: Icon.link,
  },
]

const focusClassName =
  'focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring'

const logoTileView = (
  tileClassName: string,
  iconClassName: string,
  h: HtmlBuilder<Message>,
): Html =>
  h.span(
    [
      h.Class(
        clsx(
          'grid shrink-0 place-items-center rounded-lg bg-accent text-white',
          tileClassName,
        ),
      ),
    ],
    [Blob.view(iconClassName, h)],
  )

const tooltipView = (label: string, h: HtmlBuilder<Message>): Html =>
  h.span(
    [
      h.AriaHidden(true),
      h.Class(
        'pointer-events-none absolute left-full top-1/2 z-30 max-md:hidden ml-2.5 -translate-y-1/2 whitespace-nowrap rounded-md bg-accent px-3 py-1.5 text-caption font-medium text-white opacity-0 transition-opacity duration-150 group-hover:opacity-100 group-hover:delay-300 group-focus-visible:opacity-100',
      ),
    ],
    [
      h.span([
        h.Class(
          'absolute left-0 top-1/2 size-2 -translate-x-1/2 -translate-y-1/2 rotate-45 rounded-[2px] bg-accent',
        ),
      ]),
      label,
    ],
  )

const signOutButtonView = (h: HtmlBuilder<Message>): Html =>
  h.button(
    [
      h.Type('button'),
      h.OnClick(Message.ClickedSignOut()),
      h.AriaLabel('Sign out'),
      h.Class(
        clsx(
          'group relative grid size-9 cursor-pointer place-items-center rounded-full text-mute hover:bg-hover hover:text-ink',
          focusClassName,
        ),
      ),
    ],
    [Icon.signOut('size-4'), tooltipView('Sign out', h)],
  )

const railLinkView = (item: NavItem, h: HtmlBuilder<Message>): Html =>
  h.a(
    [
      h.Href(item.href),
      h.AriaLabel(item.label),
      ...(item.isActive ? [h.AriaCurrent('page')] : []),
      h.Class(
        clsx(
          'group relative grid size-9 place-items-center rounded-full',
          focusClassName,
          item.isActive
            ? 'bg-chip text-ink'
            : 'text-mute hover:bg-hover hover:text-ink',
        ),
      ),
    ],
    [item.icon('size-4'), tooltipView(item.label, h)],
  )

const railView = (
  connection: Connection,
  page: Page,
  h: HtmlBuilder<Message>,
): Html =>
  h.header(
    [
      h.Class(
        'fixed inset-y-0 left-0 z-20 hidden w-16 flex-col items-center py-3 md:flex',
      ),
    ],
    [
      logoTileView('size-8', 'size-4', h),
      h.nav(
        [
          h.Class('mt-4 flex flex-col items-center gap-1'),
          h.AriaLabel('Settings'),
        ],
        navItems(page).map(item => railLinkView(item, h)),
      ),
      h.div(
        [h.Class('mt-auto flex flex-col items-center gap-3')],
        [statusDotView(connection, h), signOutButtonView(h)],
      ),
    ],
  )

const mobileTabView = (item: NavItem, h: HtmlBuilder<Message>): Html =>
  h.a(
    [
      h.Href(item.href),
      ...(item.isActive
        ? [h.AriaCurrent('page'), h.Id('active-nav-item')]
        : []),
      h.Class(
        clsx(
          'flex h-9 shrink-0 snap-start items-center gap-1.5 rounded-full px-3.5 text-body font-medium',
          focusClassName,
          item.isActive
            ? 'bg-chip text-ink'
            : 'text-mute hover:bg-hover hover:text-ink',
        ),
      ),
    ],
    [item.icon('size-4'), item.label],
  )

const mobileHeaderView = (
  connection: Connection,
  page: Page,
  h: HtmlBuilder<Message>,
): Html =>
  h.header(
    [
      h.Class(
        'sticky top-0 z-20 bg-shelf shadow-[0_1px_3px_rgb(0_0_0/0.06)] md:hidden',
      ),
    ],
    [
      h.div(
        [h.Class('flex h-14 items-center gap-2.5 px-4')],
        [
          logoTileView('size-7', 'size-3.5', h),
          h.p([h.Class('text-body font-semibold text-ink')], ['Blob-1']),
          statusDotView(connection, h),
          h.div([h.Class('ml-auto -mr-1.5')], [signOutButtonView(h)]),
        ],
      ),
      h.nav(
        [
          h.Class(
            'scrollbar-hide flex snap-x snap-proximity gap-1 overflow-x-auto px-4 pb-2',
          ),
          h.AriaLabel('Settings'),
        ],
        navItems(page).map(item => mobileTabView(item, h)),
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
    Chat: ({ model }) =>
      h.submodel({
        slotId: 'chat',
        model,
        view: Chat.view,
        toParentMessage: message => Message.GotChatMessage({ message }),
      }),
    Mcp: ({ model }) =>
      h.submodel({
        slotId: 'mcp',
        model,
        view: Mcp.view,
        toParentMessage: message => Message.GotMcpMessage({ message }),
      }),
  })

export const shellView = (signedIn: SignedIn, h: HtmlBuilder<Message>): Html =>
  h.div(
    [h.Class('flex min-h-dvh flex-col bg-shelf')],
    [
      railView(signedIn.connection, signedIn.page, h),
      mobileHeaderView(signedIn.connection, signedIn.page, h),
      h.main(
        [
          h.Class(
            'flex min-w-0 flex-1 flex-col md:ml-16 md:h-dvh md:max-h-dvh md:min-h-0',
          ),
        ],
        [pageView(signedIn.page, h)],
      ),
      Toast.view(
        signedIn.toast,
        message => Message.GotToastMessage({ message }),
        h,
      ),
    ],
  )
