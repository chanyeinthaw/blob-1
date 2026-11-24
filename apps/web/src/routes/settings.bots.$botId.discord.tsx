import { createFileRoute } from '@tanstack/react-router'

import { EmptyLine, PageError } from '~/components/layout/Page'
import { SplitDetail } from '~/components/layout/SplitPage'
import { useBotsRoot } from '~/features/bots/CreateBotDialog'
import {
  DiscordSettings,
  DiscordSettingsOwner,
  DiscordStatusBadge,
} from '~/features/chat/DiscordSettings'

export const Route = createFileRoute('/settings/bots/$botId/discord')({
  component: BotDiscordRoute,
})

function BotDiscordRoute() {
  const { botId } = Route.useParams()
  const bots = useBotsRoot()?.bots ?? []
  const botName = bots.find(entry => entry.id === botId)?.name ?? botId

  return (
    <DiscordSettingsOwner botId={botId}>
      {loadState => (
        <SplitDetail
          title="Discord"
          isListView={false}
          backTo="/settings/bots/$botId"
          backParams={{ botId }}
          backLabel={botName}
          {...(loadState.kind === 'loaded' ? { actions: <DiscordStatusBadge /> } : {})}
        >
          {loadState.kind === 'loaded' ? (
            <DiscordSettings />
          ) : loadState.kind === 'failed' ? (
            <PageError cause={loadState.cause} onRetry={loadState.onRetry} />
          ) : (
            <EmptyLine>Loading…</EmptyLine>
          )}
        </SplitDetail>
      )}
    </DiscordSettingsOwner>
  )
}
