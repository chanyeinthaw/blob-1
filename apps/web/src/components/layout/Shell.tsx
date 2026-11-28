import type { BotSummary } from '@blob-1/contracts'
import { useAtomMount, useAtomSet, useAtomValue } from '@effect/atom-react'
import { Link, type LinkProps, Navigate, useNavigate, useRouterState } from '@tanstack/react-router'
import { AsyncResult } from 'effect/reactivity'
import { Cloud, Cpu, Globe, LogOut, Plug, Plus, Server, type LucideIcon } from 'lucide-react'
import { type ReactNode, useEffect, useState } from 'react'
import { toast } from 'sonner'

import { BlobTile } from '~/components/BlobMark'
import { BotAvatar, botAvatarUrl } from '~/components/BotAvatar'
import { ConfirmDialog } from '~/components/ConfirmDialog'
import { Tooltip, TooltipContent, TooltipTrigger } from '~/components/ui/tooltip'
import { botsListAtom } from '~/features/bots/botsState'
import { BotsEvents } from '~/features/bots/botsMachine'
import { BotsOwner, CreateBotDialog, useBotsRoot } from '~/features/bots/CreateBotDialog'
import { mcpPageAtom } from '~/features/mcp/mcpState'
import { modelsCatalogAtom } from '~/features/models/modelsState'
import { providersPageAtom } from '~/features/providers/providersState'
import { connectionAtom, signOutAtom, unauthorizedAtom } from '~/features/session/connection'
import { clearSignedIn } from '~/lib/session'
import { ShellMachinesProvider } from '~/lib/shellMachines'
import { cn } from '~/lib/utils'

import { focusRingClassName } from './Page'

interface GlobalNavItem {
  readonly label: string
  readonly mobileLabel: string
  readonly to: NonNullable<LinkProps['to']>
  readonly icon: LucideIcon
}

const globalNavItems: ReadonlyArray<GlobalNavItem> = [
  { label: 'Providers', mobileLabel: 'Providers', to: '/settings/providers', icon: Cloud },
  { label: 'Model catalog', mobileLabel: 'Models', to: '/settings/models', icon: Cpu },
  { label: 'MCP', mobileLabel: 'MCP', to: '/settings/mcp', icon: Plug },
  { label: 'Browser', mobileLabel: 'Browser', to: '/settings/browser', icon: Globe },
  {
    label: 'Environments',
    mobileLabel: 'Environments',
    to: '/settings/environments',
    icon: Server,
  },
]

function botDotClass(bot: BotSummary): string {
  if (!bot.enabled) return 'bg-ring'
  switch (bot.connection.state) {
    case 'online':
      return 'bg-ok'
    case 'error':
      return 'bg-destructive'
    case 'connecting':
    case 'off':
      return 'bg-ring'
  }
}

function botTooltipLabel(bot: BotSummary): string {
  if (!bot.enabled) return `${bot.name} (Paused)`
  switch (bot.connection.state) {
    case 'online':
      return `${bot.name} · Online`
    case 'connecting':
      return `${bot.name} · Connecting…`
    case 'error':
      return `${bot.name} · Error`
    case 'off':
      return bot.name
  }
}

interface ConnectionInfo {
  readonly label: string
  readonly dot: string
  readonly tone: 'ok' | 'connecting' | 'offline'
}

function useConnectionInfo(): ConnectionInfo {
  const result = useAtomValue(connectionAtom)
  return AsyncResult.match(result, {
    onInitial: () => ({ label: 'Connecting…', dot: 'bg-ring', tone: 'connecting' }),
    onFailure: () => ({ label: 'Offline', dot: 'bg-destructive', tone: 'offline' }),
    onSuccess: ({ value }) =>
      value
        ? { label: 'Connected', dot: 'bg-ok', tone: 'ok' }
        : { label: 'Offline', dot: 'bg-destructive', tone: 'offline' },
  })
}

function DesktopConnectionDot() {
  const info = useConnectionInfo()
  return (
    <RailTooltip label={info.label}>
      <TooltipTrigger
        render={
          <button
            type="button"
            aria-label={info.label}
            className={cn(
              'grid size-8 cursor-default place-items-center rounded-full',
              focusRingClassName,
            )}
          />
        }
      >
        <span aria-hidden="true" className={cn('size-2 rounded-full', info.dot)} />
        <output className="sr-only">{info.label}</output>
      </TooltipTrigger>
    </RailTooltip>
  )
}

function MobileConnectionStatus() {
  const info = useConnectionInfo()
  if (info.tone === 'ok') {
    return <output className="sr-only">{info.label}</output>
  }
  return (
    <output className="inline-flex shrink-0 items-center gap-1.5 rounded-full bg-muted px-2.5 py-0.5 text-xs font-medium">
      <span aria-hidden="true" className={cn('size-1.5 rounded-full', info.dot)} />
      <span className={info.tone === 'offline' ? 'text-destructive' : 'text-muted-foreground'}>
        {info.label}
      </span>
    </output>
  )
}

function useSignOut() {
  const signOut = useAtomSet(signOutAtom, { mode: 'promise' })
  const navigate = useNavigate()
  return () => {
    signOut().then(
      () => {
        clearSignedIn()
        return navigate({ to: '/login', replace: true })
      },
      () => toast.error('Sign out failed. Try again.'),
    )
  }
}

interface RailTooltipProps {
  readonly label: string
  readonly children: ReactNode
}

/** Names an icon-only rail control on hover and keyboard focus. */
function RailTooltip(props: RailTooltipProps) {
  return (
    <Tooltip>
      {props.children}
      <TooltipContent side="right" sideOffset={10}>
        {props.label}
      </TooltipContent>
    </Tooltip>
  )
}

function useBotsRoster(): ReadonlyArray<BotSummary> {
  return useBotsRoot()?.bots ?? []
}

function activeBotIdFromPath(pathname: string): string | null {
  const match = /^\/settings\/bots\/([^/]+)/.exec(pathname)
  return match?.[1] ?? null
}

const BOT_PAGES = {
  chat: '/settings/bots/$botId/chat',
  general: '/settings/bots/$botId/general',
  models: '/settings/bots/$botId/models',
  schedules: '/settings/bots/$botId/schedules',
  webhooks: '/settings/bots/$botId/webhooks',
  secrets: '/settings/bots/$botId/secrets',
  mcp: '/settings/bots/$botId/mcp',
  skills: '/settings/bots/$botId/skills',
  discord: '/settings/bots/$botId/discord',
} as const satisfies Record<string, NonNullable<LinkProps['to']>>
type BotPage = keyof typeof BOT_PAGES

const isBotPage = (page: string | undefined): page is BotPage =>
  page !== undefined && Object.hasOwn(BOT_PAGES, page)

/** Each bot's last opened page, so switching back returns there (chat by default). */
const lastBotPage = new Map<string, BotPage>()

/** Records the bot page in `pathname`, if it is one. */
function rememberBotPage(pathname: string) {
  const match = /^\/settings\/bots\/([^/]+)\/([^/]+)/.exec(pathname)
  const [, botId, page] = match ?? []
  if (botId !== undefined && isBotPage(page)) lastBotPage.set(botId, page)
}

function botSwitchTarget(botId: string): NonNullable<LinkProps['to']> {
  return BOT_PAGES[lastBotPage.get(botId) ?? 'chat']
}

interface RailProps {
  readonly onCreateBot: () => void
}

function Rail(props: RailProps) {
  const signOut = useSignOut()
  const bots = useBotsRoster()
  const pathname = useRouterState({ select: state => state.location.pathname })
  // Idempotent: keeps the current bot's page for the links below.
  rememberBotPage(pathname)
  const activeBotId = activeBotIdFromPath(pathname)

  return (
    <header className="fixed inset-y-0 left-0 z-20 hidden w-16 flex-col items-center pt-5 pb-3 md:flex">
      <nav aria-label="Bots" className="flex flex-col items-center gap-2">
        {bots.map(bot => {
          const isActive = activeBotId === bot.id
          return (
            <RailTooltip key={bot.id} label={botTooltipLabel(bot)}>
              <TooltipTrigger
                render={
                  <Link
                    to={botSwitchTarget(bot.id)}
                    params={{ botId: bot.id }}
                    aria-label={bot.name}
                    className={cn(
                      'relative grid size-9 place-items-center rounded-xl transition-all',
                      focusRingClassName,
                      isActive
                        ? 'ring-2 ring-foreground ring-offset-2 ring-offset-shelf'
                        : 'opacity-80 hover:opacity-100',
                      !bot.enabled && 'opacity-50',
                    )}
                  />
                }
              >
                <BotAvatar
                  seed={`${bot.id}@${bot.generation}`}
                  imageUrl={botAvatarUrl(bot)}
                  className="size-9 rounded-xl"
                />
                <span
                  aria-hidden="true"
                  className={cn(
                    'absolute -right-0.5 -bottom-0.5 size-2.5 rounded-full ring-2 ring-shelf',
                    botDotClass(bot),
                  )}
                />
              </TooltipTrigger>
            </RailTooltip>
          )
        })}
        <RailTooltip label="New bot">
          <TooltipTrigger
            render={
              <button
                type="button"
                aria-label="New bot"
                onClick={props.onCreateBot}
                className={cn(
                  'grid size-9 cursor-pointer place-items-center rounded-full border border-dashed border-border text-muted-foreground transition-colors hover:border-foreground/40 hover:bg-border/40 hover:text-foreground',
                  focusRingClassName,
                )}
              />
            }
          >
            <Plus className="size-4" />
          </TooltipTrigger>
        </RailTooltip>
      </nav>

      <div aria-hidden="true" className="my-3 h-px w-6 bg-border" />

      <nav aria-label="Global settings" className="flex flex-col items-center gap-1">
        {globalNavItems.map(item => (
          <RailTooltip key={item.label} label={item.label}>
            <TooltipTrigger
              render={
                <Link
                  to={item.to}
                  aria-label={item.label}
                  className={cn(
                    'grid size-9 place-items-center rounded-full transition-colors',
                    focusRingClassName,
                  )}
                  inactiveProps={{
                    className: 'text-muted-foreground hover:bg-border/50 hover:text-foreground',
                  }}
                  activeProps={{
                    className: 'bg-background text-foreground shadow-xs ring-1 ring-border',
                  }}
                />
              }
            >
              <item.icon className="size-4" />
            </TooltipTrigger>
          </RailTooltip>
        ))}
      </nav>

      <div className="mt-auto flex flex-col items-center gap-2">
        <DesktopConnectionDot />
        <ConfirmDialog
          tooltip="Sign out"
          tooltipSide="right"
          tooltipSideOffset={10}
          trigger={
            <button
              type="button"
              aria-label="Sign out"
              className={cn(
                'grid size-9 cursor-pointer place-items-center rounded-full text-muted-foreground transition-colors hover:bg-border/50 hover:text-foreground',
                focusRingClassName,
              )}
            >
              <LogOut className="size-4" />
            </button>
          }
          title="Sign out?"
          confirmLabel="Sign out"
          confirmVariant="default"
          onConfirm={signOut}
        />
      </div>
    </header>
  )
}

interface MobileHeaderProps {
  readonly onCreateBot: () => void
}

function MobileHeader(props: MobileHeaderProps) {
  const signOut = useSignOut()
  const bots = useBotsRoster()
  const pathname = useRouterState({ select: state => state.location.pathname })
  // Idempotent: keeps the current bot's page for the links below.
  rememberBotPage(pathname)
  const activeBotId = activeBotIdFromPath(pathname)

  return (
    <>
      <header className="bg-shelf pt-[env(safe-area-inset-top)] md:hidden">
        <div className="flex h-12 items-center gap-2.5 pr-[max(1rem,env(safe-area-inset-right))] pl-[max(1rem,env(safe-area-inset-left))]">
          <BlobTile className="size-7" />
          <p className="text-sm font-semibold">Blob-1</p>
          <MobileConnectionStatus />
          <div className="-mr-1.5 ml-auto">
            <ConfirmDialog
              trigger={
                <button
                  type="button"
                  aria-label="Sign out"
                  className={cn(
                    'grid size-10 cursor-pointer place-items-center rounded-full text-muted-foreground transition-colors hover:bg-border/50 hover:text-foreground',
                    focusRingClassName,
                  )}
                >
                  <LogOut className="size-4" />
                </button>
              }
              title="Sign out?"
              confirmLabel="Sign out"
              confirmVariant="default"
              onConfirm={signOut}
            />
          </div>
        </div>
      </header>
      <div className="sticky top-0 z-20 border-b border-border/70 bg-shelf md:hidden">
        <nav
          aria-label="Bots and global settings"
          className="scrollbar-hide flex items-center gap-1 overflow-x-auto py-2 pr-[max(0.75rem,env(safe-area-inset-right))] pl-[max(0.75rem,env(safe-area-inset-left))]"
        >
          {bots.map(bot => {
            const isActive = activeBotId === bot.id
            return (
              <Link
                key={bot.id}
                to={botSwitchTarget(bot.id)}
                params={{ botId: bot.id }}
                className={cn(
                  'flex h-9 shrink-0 items-center gap-1.5 rounded-full px-3 text-xs font-medium transition-colors',
                  focusRingClassName,
                  isActive
                    ? 'bg-background text-foreground shadow-xs ring-1 ring-border'
                    : 'text-muted-foreground hover:bg-border/50 hover:text-foreground',
                )}
              >
                <span
                  aria-hidden="true"
                  className={cn('size-1.5 rounded-full', botDotClass(bot))}
                />
                {bot.name}
              </Link>
            )
          })}
          <button
            type="button"
            aria-label="New bot"
            onClick={props.onCreateBot}
            className={cn(
              'grid size-8 shrink-0 cursor-pointer place-items-center rounded-full border border-dashed border-border text-muted-foreground',
              focusRingClassName,
            )}
          >
            <Plus className="size-3.5" />
          </button>
          <span aria-hidden="true" className="mx-1 h-4 w-px shrink-0 bg-border" />
          {globalNavItems.map(item => (
            <Link
              key={item.label}
              to={item.to}
              className={cn(
                'flex h-9 shrink-0 items-center gap-1.5 rounded-full px-3 text-xs font-medium transition-colors',
                focusRingClassName,
              )}
              inactiveProps={{
                className: 'text-muted-foreground hover:bg-border/50 hover:text-foreground',
              }}
              activeProps={{
                className: 'bg-background text-foreground shadow-xs ring-1 ring-border',
              }}
            >
              <item.icon className="size-3.5" />
              {item.mobileLabel}
            </Link>
          ))}
        </nav>
      </div>
    </>
  )
}

function GlobalPrefetch() {
  useAtomMount(botsListAtom)
  useAtomMount(providersPageAtom)
  useAtomMount(modelsCatalogAtom)
  useAtomMount(mcpPageAtom)
  return null
}

function UnauthorizedRedirect() {
  const unauthorized = AsyncResult.isSuccess(useAtomValue(unauthorizedAtom))
  useEffect(() => {
    if (unauthorized) clearSignedIn()
  }, [unauthorized])
  return unauthorized ? <Navigate to="/login" replace /> : null
}

const DEFAULT_FALLBACK_BOTS: ReadonlyArray<BotSummary> = [
  {
    id: 'default',
    name: 'Blob-1',
    enabled: true,
    isDefault: true,
    createdAt: 0,
    generation: 0,
    avatarAt: null,
    mainModel: null,
    connection: { state: 'off' },
    environment: 'local',
    environmentOnline: true,
  },
]

interface ShellInnerProps {
  readonly children: ReactNode
}

function BotsPoller() {
  const machine = BotsOwner.useMachine()
  const send = useAtomSet(machine.send)

  useEffect(() => {
    send(BotsEvents.Refresh())
    const interval = window.setInterval(() => {
      if (document.visibilityState === 'visible') {
        send(BotsEvents.Refresh())
      }
    }, 5000)
    const onFocus = () => send(BotsEvents.Refresh())
    window.addEventListener('focus', onFocus)
    return () => {
      window.clearInterval(interval)
      window.removeEventListener('focus', onFocus)
    }
  }, [send])

  return null
}

function ShellInner(props: ShellInnerProps) {
  const [createOpen, setCreateOpen] = useState(false)
  return (
    <div className="flex min-h-dvh flex-col bg-shelf">
      <GlobalPrefetch />
      <BotsPoller />
      <Rail onCreateBot={() => setCreateOpen(true)} />
      <MobileHeader onCreateBot={() => setCreateOpen(true)} />
      <main className="flex min-w-0 flex-1 flex-col md:ml-16 md:h-dvh md:max-h-dvh md:min-h-0 md:py-2">
        {props.children}
      </main>
      <CreateBotDialog open={createOpen} onOpenChange={setCreateOpen} />
      <UnauthorizedRedirect />
    </div>
  )
}

interface ShellProps {
  readonly children: ReactNode
}

/** The signed-in frame: bot & global icon rail on desktop, bot/global strip on mobile. */
export function Shell(props: ShellProps) {
  const botsResult = useAtomValue(botsListAtom)
  const initialBots = AsyncResult.match(botsResult, {
    onInitial: () => DEFAULT_FALLBACK_BOTS,
    onFailure: () => DEFAULT_FALLBACK_BOTS,
    onSuccess: ({ value }) => value,
  })
  return (
    <ShellMachinesProvider>
      <BotsOwner.Provider
        input={initialBots}
        scopeKey={AsyncResult.isSuccess(botsResult) ? 'loaded' : 'initial'}
      >
        <ShellInner>{props.children}</ShellInner>
      </BotsOwner.Provider>
    </ShellMachinesProvider>
  )
}
