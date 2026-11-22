import { createFileRoute } from '@tanstack/react-router'

import { SignInScreen } from '~/features/session/SignInScreen'
import { skipSignInWhenSignedIn } from '~/lib/session'

export const Route = createFileRoute('/login')({
  beforeLoad: skipSignInWhenSignedIn,
  component: SignInScreen,
})
