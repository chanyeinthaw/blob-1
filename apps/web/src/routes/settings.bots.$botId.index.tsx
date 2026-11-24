import { createFileRoute } from '@tanstack/react-router'

import { GeneralPage } from '~/features/general/GeneralPage'

export const Route = createFileRoute('/settings/bots/$botId/')({
  component: BotIndexRoute,
})

function BotIndexRoute() {
  const { botId } = Route.useParams()
  return <GeneralPage botId={botId} isListView />
}
