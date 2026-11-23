import { createRootRoute, Navigate, Outlet } from '@tanstack/react-router'

export const Route = createRootRoute({ component: Outlet, notFoundComponent: NotFound })

/** Unknown addresses land on General, or the sign-in screen when signed out. */
function NotFound() {
  return <Navigate to="/" replace />
}
