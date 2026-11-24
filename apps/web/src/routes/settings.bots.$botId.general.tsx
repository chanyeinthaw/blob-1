import { createFileRoute } from '@tanstack/react-router'

import { GeneralPage } from '~/features/general/GeneralPage'

export const Route = createFileRoute('/settings/bots/$botId/general')({
  component: BotGeneralRoute,
})

function BotGeneralRoute() {
  const { botId } = Route.useParams()
  return <GeneralPage botId={botId} />
}
