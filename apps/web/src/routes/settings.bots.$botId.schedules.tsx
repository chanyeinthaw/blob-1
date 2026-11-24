import { createFileRoute } from '@tanstack/react-router'

import { SchedulesPage } from '~/features/schedules/SchedulesPage'

export const Route = createFileRoute('/settings/bots/$botId/schedules')({
  component: BotSchedulesRoute,
})

function BotSchedulesRoute() {
  const { botId } = Route.useParams()
  return <SchedulesPage botId={botId} />
}
