import { createFileRoute, Outlet } from '@tanstack/react-router'

import { Shell } from '~/components/layout/Shell'
import { requireSession } from '~/lib/session'

export const Route = createFileRoute('/settings')({
  beforeLoad: requireSession,
  component: SettingsLayout,
})

function SettingsLayout() {
  return (
    <Shell>
      <Outlet />
    </Shell>
  )
}
