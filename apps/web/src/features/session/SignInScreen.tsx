import { useAtomSet } from '@effect/atom-react'
import { AtomMachine } from '@typeonce/effect-machine/reactivity'
import {
  createMachineContext,
  MachineState,
  useMachineResult,
} from '@typeonce/effect-machine-react'
import { Navigate } from '@tanstack/react-router'
import { Option } from 'effect'
import { AsyncResult } from 'effect/reactivity'
import { useEffect, useRef, useState } from 'react'

import { BlobTile } from '~/components/BlobMark'
import { FieldError } from '~/components/layout/Page'
import { Button } from '~/components/ui/button'
import { Input } from '~/components/ui/input'
import { MachineAtoms } from '~/lib/runtime'
import { markSignedIn } from '~/lib/session'

import { SignInEvents, SignInMachine } from './signIn'

const SignIn = createMachineContext(MachineAtoms.factory(SignInMachine))

export function SignInScreen() {
  return (
    <SignIn.Provider>
      <div className="grid min-h-dvh place-items-center bg-shelf px-5 pt-[max(4rem,env(safe-area-inset-top))] pb-[max(4rem,env(safe-area-inset-bottom))]">
        <div className="flex w-full max-w-[340px] flex-col gap-6 rounded-2xl border border-border bg-background p-6 shadow-xs">
          <div className="flex flex-col items-center gap-3">
            <BlobTile className="size-9" />
            <h1 className="text-xl font-semibold">Sign in to Blob-1</h1>
          </div>
          <SignInForm />
        </div>
      </div>
    </SignIn.Provider>
  )
}

function SignedInRedirect() {
  markSignedIn()
  return <Navigate to="/settings/bots/$botId/chat" params={{ botId: 'default' }} replace />
}

function SignInForm() {
  const machine = SignIn.useMachine()
  const send = useAtomSet(machine.send)
  const [password, setPassword] = useState('')
  const passwordRef = useRef<HTMLInputElement>(null)
  const editing = Option.getOrNull(
    AsyncResult.getOrElse(useMachineResult(AtomMachine.select(machine, 'Editing')), () =>
      Option.none(),
    ),
  )
  const isSubmitting = AsyncResult.getOrElse(
    useMachineResult(AtomMachine.matches(machine, 'Submitting')),
    () => false,
  )
  const error = editing?.error ?? null

  useEffect(() => {
    if (window.matchMedia('(pointer: fine)').matches) {
      passwordRef.current?.focus()
    }
  }, [])

  useEffect(() => {
    if (error !== null) {
      passwordRef.current?.focus()
      passwordRef.current?.select()
    }
  }, [error])

  return (
    <form
      noValidate
      className="flex flex-col gap-3"
      onSubmit={event => {
        event.preventDefault()
        send(SignInEvents.Submit({ password }))
      }}
    >
      <div>
        <label htmlFor="password" className="sr-only">
          Password
        </label>
        <Input
          ref={passwordRef}
          id="password"
          type="password"
          autoComplete="current-password"
          placeholder="Password"
          value={password}
          readOnly={isSubmitting}
          aria-invalid={error !== null}
          aria-describedby={error === null ? undefined : 'signin-error'}
          onChange={event => {
            setPassword(event.target.value)
            if (error !== null) send(SignInEvents.ClearError())
          }}
        />
        {error === null ? null : <FieldError id="signin-error">{error}</FieldError>}
      </div>
      <MachineState machine={machine} path="SignedIn" inactive={null}>
        {() => <SignedInRedirect />}
      </MachineState>
      <Button type="submit" disabled={isSubmitting || password === ''}>
        {isSubmitting ? 'Signing in…' : 'Sign in'}
      </Button>
    </form>
  )
}
