import { createFileRoute } from '@tanstack/react-router'

import { WebhooksPage } from '~/features/webhooks/WebhooksPage'

export const Route = createFileRoute('/settings/webhooks')({
  component: WebhooksPage,
})
