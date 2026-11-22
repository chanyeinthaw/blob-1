import { createFileRoute, useMatchRoute } from '@tanstack/react-router'

import { ChatPlatformsPage } from '~/features/chat/ChatPlatformsPage'

export const Route = createFileRoute('/settings/chat')({
  component: ChatPlatformsRoute,
})

/** Desktop shows the list and Discord together; on mobile `/settings/chat` is the list alone. */
function ChatPlatformsRoute() {
  const matchRoute = useMatchRoute()
  return <ChatPlatformsPage isListView={matchRoute({ to: '/settings/chat' }) !== false} />
}
