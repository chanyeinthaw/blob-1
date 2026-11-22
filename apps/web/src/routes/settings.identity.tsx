import { createFileRoute } from '@tanstack/react-router'

import { IdentityPage } from '~/features/identity/IdentityPage'

export const Route = createFileRoute('/settings/identity')({
  component: IdentityPage,
})
