import { createFileRoute, redirect } from '@tanstack/react-router'

export const Route = createFileRoute('/settings/schedules')({
  beforeLoad: () => {
    throw redirect({ to: '/settings/bots/$botId/schedules', params: { botId: 'default' } })
  },
})
