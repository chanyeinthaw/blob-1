import { createFileRoute, redirect } from '@tanstack/react-router'

export const Route = createFileRoute('/settings/general')({
  beforeLoad: () => {
    throw redirect({ to: '/settings/bots/$botId/general', params: { botId: 'default' } })
  },
})
