import { useAtomMount, useAtomSet, useAtomValue } from '@effect/atom-react'
import { Link, type LinkProps, Navigate, useNavigate } from '@tanstack/react-router'
import { AsyncResult } from 'effect/reactivity'
import {
  CalendarClock,
  Cloud,
  Cpu,
  Home,
  LogOut,
  MessagesSquare,
  Plug,
  Zap,
  type LucideIcon,
} from 'lucide-react'
import { type ReactNode, useEffect } from 'react'
import { toast } from 'sonner'

import { BlobTile } from '~/components/BlobMark'
import { ConfirmDialog } from '~/components/ConfirmDialog'
import { Tooltip, TooltipContent, TooltipTrigger } from '~/components/ui/tooltip'
import { discordPageAtom } from '~/features/chat/discordState'
import { generalPageAtom } from '~/features/general/generalState'
import { usageAtomFor } from '~/features/general/usageState'
import { mcpPageAtom } from '~/features/mcp/mcpState'
import { modelsPageAtom } from '~/features/models/modelsState'
import { providersPageAtom } from '~/features/providers/providersState'
import { schedulesPageAtom } from '~/features/schedules/schedulesState'
import { connectionAtom, signOutAtom, unauthorizedAtom } from '~/features/session/connection'
import { webhooksPageAtom } from '~/features/webhooks/webhooksState'
import { clearSignedIn } from '~/lib/session'
import { ShellMachinesProvider } from '~/lib/shellMachines'
import { cn } from '~/lib/utils'

import { focusRingClassName } from './Page'

interface NavItem {
  readonly label: string
  readonly mobileLabel: string
  readonly to: NonNullable<LinkProps['to']>
  readonly icon: LucideIcon
}

const navItems: ReadonlyArray<NavItem> = [
  { label: 'General', mobileLabel: 'General', to: '/settings/general', icon: Home },
  { label: 'Schedules', mobileLabel: 'Schedules', to: '/settings/schedules', icon: CalendarClock },
  { label: 'Chat platforms', mobileLabel: 'Chat', to: '/settings/chat', icon: MessagesSquare },
  { label: 'MCP', mobileLabel: 'MCP', to: '/settings/mcp', icon: Plug },
  { label: 'Webhooks', mobileLabel: 'Webhooks', to: '/settings/webhooks', icon: Zap },
  { label: 'Models', mobileLabel: 'Models', to: '/settings/models', icon: Cpu },
  { label: 'Providers', mobileLabel: 'Providers', to: '/settings/providers', icon: Cloud },
]

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

function Rail() {
  const signOut = useSignOut()
  return (
    <header className="fixed inset-y-0 left-0 z-20 hidden w-16 flex-col items-center py-3 md:flex">
      <BlobTile />
      <nav aria-label="Settings" className="mt-4 flex flex-col items-center gap-1">
        {navItems.map(item => (
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

function MobileHeader() {
  const signOut = useSignOut()
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
          aria-label="Settings"
          className="scrollbar-hide flex items-center justify-between gap-0.5 overflow-x-auto py-2 pr-[max(0.75rem,env(safe-area-inset-right))] pl-[max(0.75rem,env(safe-area-inset-left))] min-[375px]:gap-1 min-[375px]:pr-[max(1rem,env(safe-area-inset-right))] min-[375px]:pl-[max(1rem,env(safe-area-inset-left))] sm:justify-start sm:gap-1.5"
        >
          {navItems.map(item => (
            <Link
              key={item.label}
              to={item.to}
              {...(item.label !== item.mobileLabel ? { 'aria-label': item.label } : {})}
              className={cn(
                'flex h-9 shrink-0 items-center gap-1.5 rounded-full px-2 text-xs font-medium transition-colors min-[375px]:px-2.5 min-[375px]:text-[13px] sm:px-3.5 sm:text-sm',
                focusRingClassName,
              )}
              inactiveProps={{
                className: 'text-muted-foreground hover:bg-border/50 hover:text-foreground',
              }}
              activeProps={{
                className: 'bg-background text-foreground shadow-xs ring-1 ring-border',
              }}
            >
              <item.icon className="hidden size-4 sm:block" />
              {item.mobileLabel}
            </Link>
          ))}
        </nav>
      </div>
    </>
  )
}

function SettingsPrefetch() {
  useAtomMount(generalPageAtom)
  useAtomMount(usageAtomFor(7))
  useAtomMount(schedulesPageAtom)
  useAtomMount(discordPageAtom)
  useAtomMount(mcpPageAtom)
  useAtomMount(webhooksPageAtom)
  useAtomMount(modelsPageAtom)
  useAtomMount(providersPageAtom)
  return null
}

function UnauthorizedRedirect() {
  const unauthorized = AsyncResult.isSuccess(useAtomValue(unauthorizedAtom))
  useEffect(() => {
    if (unauthorized) clearSignedIn()
  }, [unauthorized])
  return unauthorized ? <Navigate to="/login" replace /> : null
}

interface ShellProps {
  readonly children: ReactNode
}

/** The signed-in frame: icon rail on desktop, header with a sticky tab strip on mobile. */
export function Shell(props: ShellProps) {
  return (
    <ShellMachinesProvider>
      <div className="flex min-h-dvh flex-col bg-shelf">
        <SettingsPrefetch />
        <Rail />
        <MobileHeader />
        <main className="flex min-w-0 flex-1 flex-col md:ml-16 md:h-dvh md:max-h-dvh md:min-h-0">
          {props.children}
        </main>
        <UnauthorizedRedirect />
      </div>
    </ShellMachinesProvider>
  )
}
