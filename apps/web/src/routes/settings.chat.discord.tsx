import { createFileRoute, redirect } from '@tanstack/react-router'

export const Route = createFileRoute('/settings/chat/discord')({
  beforeLoad: () => {
    throw redirect({ to: '/settings/bots/$botId/discord', params: { botId: 'default' } })
  },
})
