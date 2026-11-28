import { createFileRoute } from '@tanstack/react-router'

import { BotSkillsPage } from '~/features/bots/BotSkillsPage'

export const Route = createFileRoute('/settings/bots/$botId/skills')({
  component: BotSkillsRoute,
})

function BotSkillsRoute() {
  const { botId } = Route.useParams()
  return <BotSkillsPage botId={botId} />
}
