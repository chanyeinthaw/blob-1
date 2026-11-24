import type { BotSummary } from '@blob-1/contracts'
import { useAtomMount, useAtomSet } from '@effect/atom-react'
import { Outlet, useNavigate, useRouterState } from '@tanstack/react-router'
import { CalendarClock, Cpu, Home, KeyRound, Plug, Zap } from 'lucide-react'
import { useEffect } from 'react'

import { DiscordIcon } from '~/components/DiscordIcon'
import { SplitGroupLabel, SplitItem, SplitPage } from '~/components/layout/SplitPage'
import { Badge } from '~/components/ui/badge'
import { discordPageAtomFor } from '~/features/chat/discordState'
import { generalPageAtomFor } from '~/features/general/generalState'
import { usageAtomFor } from '~/features/general/usageState'
import { modelsPageAtomFor } from '~/features/models/modelsState'
import { schedulesPageAtomFor } from '~/features/schedules/schedulesState'
import { secretsPageAtomFor } from '~/features/secrets/secretsState'
import { webhooksPageAtomFor } from '~/features/webhooks/webhooksState'
import { cn } from '~/lib/utils'

import { BotsEvents } from './botsMachine'
import { botMcpAtomFor } from './botsState'
import { BotsOwner, useBotInstanceKey, useBotsRoot } from './CreateBotDialog'

function dotClassFor(bot: BotSummary | undefined): string {
  if (bot === undefined || !bot.enabled) return 'bg-ring'
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

function BotPrefetch(props: { readonly instanceKey: string }) {
  useAtomMount(generalPageAtomFor(props.instanceKey))
  useAtomMount(usageAtomFor(props.instanceKey, 7))
  useAtomMount(modelsPageAtomFor(props.instanceKey))
  useAtomMount(schedulesPageAtomFor(props.instanceKey))
  useAtomMount(webhooksPageAtomFor(props.instanceKey))
  useAtomMount(secretsPageAtomFor(props.instanceKey))
  useAtomMount(discordPageAtomFor(props.instanceKey))
  useAtomMount(botMcpAtomFor(props.instanceKey))
  return null
}

interface BotShellPageProps {
  readonly botId: string
}

/** Nested SplitPage sidebar for a single bot: General, Models, Schedules, Webhooks, MCP, and Platforms → Discord. */
export function BotShellPage(props: BotShellPageProps) {
  const root = useBotsRoot()
  const bots = root?.bots ?? []
  const instanceKey = useBotInstanceKey(props.botId)
  const pathname = useRouterState({ select: state => state.location.pathname })
  const send = useAtomSet(BotsOwner.useMachine().send)
  const navigate = useNavigate()

  const bot = bots.find(entry => entry.id === props.botId)
  const botName = bot?.name ?? props.botId
  const lastRemovedId = root?.lastRemovedId ?? null

  // Deletion navigation lives here (above the instance-keyed Outlet) so a bot
  // disappearing from the roster cannot unmount the effect before it navigates.
  useEffect(() => {
    if (lastRemovedId !== props.botId) return
    send(BotsEvents.ClearRemovedState())
    void navigate({
      to: '/settings/bots/$botId/general',
      params: { botId: 'default' },
      replace: true,
    })
  }, [lastRemovedId, navigate, props.botId, send])

  const base = `/settings/bots/${props.botId}`
  const isListView = pathname === base || pathname === `${base}/`
  const isActive = (segment: string) =>
    pathname === `${base}/${segment}` || pathname.startsWith(`${base}/${segment}/`)

  return (
    <>
      <BotPrefetch instanceKey={instanceKey} />
      <SplitPage
        title={botName}
        titleExtra={
          bot !== undefined && !bot.enabled ? (
            <Badge variant="outline" className="shrink-0 text-muted-foreground">
              Paused
            </Badge>
          ) : null
        }
        isListView={isListView}
        items={
          <>
            <SplitItem
              to="/settings/bots/$botId/general"
              params={{ botId: props.botId }}
              label="General"
              icon={<Home className="size-4 shrink-0 text-muted-foreground" />}
              isActive={isActive('general')}
            />
            <SplitItem
              to="/settings/bots/$botId/models"
              params={{ botId: props.botId }}
              label="Models"
              icon={<Cpu className="size-4 shrink-0 text-muted-foreground" />}
              isActive={isActive('models')}
            />
            <SplitItem
              to="/settings/bots/$botId/schedules"
              params={{ botId: props.botId }}
              label="Schedules"
              icon={<CalendarClock className="size-4 shrink-0 text-muted-foreground" />}
              isActive={isActive('schedules')}
            />
            <SplitItem
              to="/settings/bots/$botId/webhooks"
              params={{ botId: props.botId }}
              label="Webhooks"
              icon={<Zap className="size-4 shrink-0 text-muted-foreground" />}
              isActive={isActive('webhooks')}
            />
            <SplitItem
              to="/settings/bots/$botId/secrets"
              params={{ botId: props.botId }}
              label="Secrets"
              icon={<KeyRound className="size-4 shrink-0 text-muted-foreground" />}
              isActive={isActive('secrets')}
            />
            <SplitItem
              to="/settings/bots/$botId/mcp"
              params={{ botId: props.botId }}
              label="MCP"
              icon={<Plug className="size-4 shrink-0 text-muted-foreground" />}
              isActive={isActive('mcp')}
            />
            <SplitGroupLabel>Platforms</SplitGroupLabel>
            <SplitItem
              to="/settings/bots/$botId/discord"
              params={{ botId: props.botId }}
              label="Discord"
              icon={<DiscordIcon className="size-4 shrink-0 text-[#5865F2]" />}
              isActive={isActive('discord')}
              trailing={
                <span
                  aria-hidden="true"
                  className={cn('size-2 shrink-0 rounded-full', dotClassFor(bot))}
                />
              }
            />
          </>
        }
      >
        <Outlet key={instanceKey} />
      </SplitPage>
    </>
  )
}
