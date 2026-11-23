import { createFileRoute } from '@tanstack/react-router'

import { GeneralPage } from '~/features/general/GeneralPage'

export const Route = createFileRoute('/settings/general')({
  component: GeneralPage,
})
