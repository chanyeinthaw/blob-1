import { createFileRoute } from '@tanstack/react-router'

import { SplitDetail } from '~/components/layout/SplitPage'
import { BotMcpBody } from '~/features/bots/BotMcpPage'
import { useBotsRoot } from '~/features/bots/CreateBotDialog'

export const Route = createFileRoute('/settings/bots/$botId/mcp')({
  component: BotMcpRoute,
})

function BotMcpRoute() {
  const { botId } = Route.useParams()
  const bots = useBotsRoot()?.bots ?? []
  const botName = bots.find(entry => entry.id === botId)?.name ?? botId
  return (
    <SplitDetail
      title="MCP"
      isListView={false}
      backTo="/settings/bots/$botId"
      backParams={{ botId }}
      backLabel={botName}
    >
      <BotMcpBody botId={botId} />
    </SplitDetail>
  )
}
