import { createFileRoute } from '@tanstack/react-router'

import { ProvidersPage } from '~/features/providers/ProvidersPage'

export const Route = createFileRoute('/settings/providers')({
  component: ProvidersPage,
})
