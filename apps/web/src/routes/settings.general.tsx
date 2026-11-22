import { createFileRoute } from '@tanstack/react-router'

import { GeneralPage } from '~/features/identity/IdentityPage'

export const Route = createFileRoute('/settings/general')({
  component: GeneralPage,
})
