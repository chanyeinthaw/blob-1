import { createFileRoute } from '@tanstack/react-router'

import { BotShellPage } from '~/features/bots/BotShellPage'

export const Route = createFileRoute('/settings/bots/$botId')({
  component: BotShellRoute,
})

function BotShellRoute() {
  const { botId } = Route.useParams()
  return <BotShellPage botId={botId} />
}
