import { createFileRoute, redirect } from '@tanstack/react-router'

export const Route = createFileRoute('/settings/discord')({
  beforeLoad: () => {
    throw redirect({ to: '/settings/chat/discord', replace: true })
  },
})
