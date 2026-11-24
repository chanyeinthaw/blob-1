import { createFileRoute } from '@tanstack/react-router'

import { SecretsPage } from '~/features/secrets/SecretsPage'

export const Route = createFileRoute('/settings/bots/$botId/secrets')({
  component: BotSecretsRoute,
})

function BotSecretsRoute() {
  const { botId } = Route.useParams()
  return <SecretsPage botId={botId} />
}
