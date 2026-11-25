import { createFileRoute } from '@tanstack/react-router'

import { WebChatPage } from '~/features/webchat/WebChatPage'

export const Route = createFileRoute('/settings/bots/$botId/chat')({
  component: BotChatRoute,
})

function BotChatRoute() {
  const { botId } = Route.useParams()
  return <WebChatPage botId={botId} />
}
