import { createFileRoute, redirect } from '@tanstack/react-router'

export const Route = createFileRoute('/settings/webhooks')({
  beforeLoad: () => {
    throw redirect({ to: '/settings/bots/$botId/webhooks', params: { botId: 'default' } })
  },
})
