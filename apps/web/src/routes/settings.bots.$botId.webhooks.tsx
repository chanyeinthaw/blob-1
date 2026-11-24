import { createFileRoute } from '@tanstack/react-router'

import { WebhooksPage } from '~/features/webhooks/WebhooksPage'

export const Route = createFileRoute('/settings/bots/$botId/webhooks')({
  component: BotWebhooksRoute,
})

function BotWebhooksRoute() {
  const { botId } = Route.useParams()
  return <WebhooksPage botId={botId} />
}
