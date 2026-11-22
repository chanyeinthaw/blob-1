import { RegistryProvider } from '@effect/atom-react'
import { createRouter, RouterProvider } from '@tanstack/react-router'
import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'

import { Toaster } from './components/ui/sonner'
import { TooltipProvider } from './components/ui/tooltip'
import { routeTree } from './routeTree.gen'
import './styles.css'

const router = createRouter({ routeTree, defaultPreload: 'intent' })

declare module '@tanstack/react-router' {
  interface Register {
    router: typeof router
  }
}

const container = document.getElementById('root')
if (container !== null) {
  createRoot(container).render(
    <StrictMode>
      <RegistryProvider>
        <TooltipProvider delay={300}>
          <RouterProvider router={router} />
          <Toaster />
        </TooltipProvider>
      </RegistryProvider>
    </StrictMode>,
  )
}
