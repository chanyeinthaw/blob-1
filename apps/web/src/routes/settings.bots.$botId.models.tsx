import { createFileRoute } from '@tanstack/react-router'

import { BotModelsPage } from '~/features/models/ModelsPage'

export const Route = createFileRoute('/settings/bots/$botId/models')({
  component: BotModelsRoute,
})

function BotModelsRoute() {
  const { botId } = Route.useParams()
  return <BotModelsPage botId={botId} />
}
