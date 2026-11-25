import { createFileRoute, redirect } from '@tanstack/react-router'

export const Route = createFileRoute('/')({
  beforeLoad: () => {
    throw redirect({
      to: '/settings/bots/$botId/chat',
      params: { botId: 'default' },
      replace: true,
    })
  },
})
