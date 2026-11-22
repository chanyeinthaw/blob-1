import { useAtomRefresh, useAtomSet, useAtomValue } from '@effect/atom-react'
import { useBlocker } from '@tanstack/react-router'
import { AtomMachine } from '@typeonce/effect-machine/reactivity'
import {
  createMachineContext,
  MachineState,
  useMachineResult,
} from '@typeonce/effect-machine-react'
import { Option } from 'effect'
import { AsyncResult } from 'effect/reactivity'
import { Undo2 } from 'lucide-react'
import { useEffect } from 'react'

import { ConfirmDialog } from '~/components/ConfirmDialog'
import { EmptyLine, Page, PageError } from '~/components/layout/Page'
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '~/components/ui/alert-dialog'
import { Button } from '~/components/ui/button'
import { MachineAtoms } from '~/lib/runtime'
import { cn } from '~/lib/utils'

import { IdentityEvents, type IdentityInput, IdentityMachine } from './identityMachine'
import { identityPageAtom } from './identityState'

const IdentityOwner = createMachineContext(({ input }: { input: IdentityInput }) =>
  MachineAtoms.make(IdentityMachine, input),
)

const resetAllowed = AtomMachine.can(IdentityEvents.Reset())

const textClassName =
  'col-start-1 row-start-1 min-h-56 w-full px-3.5 py-3 whitespace-pre-wrap break-words text-base leading-relaxed text-foreground md:text-sm'

/** The identity settings page: autosaving textarea with a reset-to-default action. */
export function IdentityPage() {
  const result = useAtomValue(identityPageAtom)
  const retry = useAtomRefresh(identityPageAtom)
  return AsyncResult.match(result, {
    onInitial: () => (
      <Page title="Identity">
        <EmptyLine>Loading…</EmptyLine>
      </Page>
    ),
    onFailure: failure => (
      <Page title="Identity">
        <PageError cause={failure.cause} onRetry={retry} />
      </Page>
    ),
    onSuccess: ({ value }) => (
      <IdentityOwner.Provider input={value}>
        <Page title="Identity" actions={<IdentityActions />}>
          <IdentityEditor />
          <UnsavedGuard />
        </Page>
      </IdentityOwner.Provider>
    ),
  })
}

function IdentityActions() {
  const machine = IdentityOwner.useMachine()
  const send = useAtomSet(machine.send)
  const canReset = AsyncResult.match(useMachineResult(resetAllowed(machine)), {
    onInitial: () => false,
    onFailure: () => false,
    onSuccess: ({ value }) => value,
  })
  return (
    <>
      <span
        aria-live="polite"
        className="mr-1 hidden text-xs text-muted-foreground empty:sr-only md:inline"
      >
        <MachineState machine={machine} path="Saving">
          {() => 'Saving…'}
        </MachineState>
        <MachineState machine={machine} path="Resetting">
          {() => 'Resetting…'}
        </MachineState>
        <MachineState machine={machine} path="">
          {({ value, state }) =>
            state.path === 'Idle' && value.hasSaved && value.error === null ? 'Saved' : null
          }
        </MachineState>
      </span>
      <MachineState machine={machine} path="">
        {({ value }) =>
          value.isDefault ? null : (
            <ConfirmDialog
              tooltip="Reset to default"
              trigger={
                <Button
                  variant="ghost"
                  size="icon"
                  aria-label="Reset to default"
                  disabled={!canReset}
                >
                  <Undo2 />
                </Button>
              }
              title="Reset identity?"
              detail="This replaces your custom identity with the default."
              confirmLabel="Reset"
              onConfirm={() => send(IdentityEvents.Reset())}
            />
          )
        }
      </MachineState>
    </>
  )
}

interface IdentityFooterProps {
  readonly error: string | null
  readonly failedAction: 'save' | 'reset' | null
  readonly hasSaved: boolean
  readonly canRetrySave: boolean
  readonly isSaving: boolean
  readonly isResetting: boolean
  readonly onRetrySave: () => void
  readonly onRetryReset: () => void
}

function IdentityFooter(props: IdentityFooterProps) {
  const mobileStatus =
    props.error !== null
      ? null
      : props.isSaving
        ? 'Saving…'
        : props.isResetting
          ? 'Resetting…'
          : props.hasSaved
            ? 'Saved'
            : null
  return (
    <>
      {props.error === null ? null : (
        <div className="mt-2 flex flex-wrap items-center gap-3">
          <p
            id="identity-error"
            role="alert"
            className="break-words text-xs text-destructive [overflow-wrap:anywhere]"
          >
            {props.error}
          </p>
          {props.failedAction === 'save' && props.canRetrySave && !props.isSaving ? (
            <Button type="button" variant="outline" size="sm" onClick={props.onRetrySave}>
              Retry save
            </Button>
          ) : null}
          {props.failedAction === 'reset' && !props.isResetting ? (
            <Button type="button" variant="outline" size="sm" onClick={props.onRetryReset}>
              Retry reset
            </Button>
          ) : null}
        </div>
      )}
      <span
        aria-live="polite"
        className="mt-2 text-xs text-muted-foreground empty:sr-only md:hidden"
      >
        {mobileStatus}
      </span>
    </>
  )
}

function IdentityEditor() {
  const machine = IdentityOwner.useMachine()
  const send = useAtomSet(machine.send)
  const isResetting = AsyncResult.match(
    useMachineResult(AtomMachine.matches('Resetting')(machine)),
    {
      onInitial: () => false,
      onFailure: () => false,
      onSuccess: ({ value }) => value,
    },
  )
  const isSaving = AsyncResult.match(useMachineResult(AtomMachine.matches('Saving')(machine)), {
    onInitial: () => false,
    onFailure: () => false,
    onSuccess: ({ value }) => value,
  })
  return (
    <MachineState machine={machine} path="">
      {({ value }) => (
        <div className="flex flex-col">
          <div
            className={cn(
              'grid grid-cols-1 rounded-2xl border bg-background focus-within:ring-3 focus-within:ring-inset focus-within:ring-ring/50',
              value.error === null ? 'border-input focus-within:border-ring' : 'border-destructive',
            )}
          >
            <div aria-hidden="true" className={cn(textClassName, 'invisible')}>
              {value.draftText}{' '}
            </div>
            <textarea
              aria-label="Identity"
              aria-invalid={value.error !== null}
              aria-describedby={value.error === null ? undefined : 'identity-error'}
              className={cn(
                textClassName,
                'h-full resize-none overflow-hidden bg-transparent caret-foreground outline-none disabled:cursor-not-allowed disabled:opacity-50',
              )}
              value={value.draftText}
              disabled={isResetting}
              onChange={event => send(IdentityEvents.Edit({ text: event.target.value }))}
              onBlur={() => send(IdentityEvents.Flush())}
            />
          </div>
          <IdentityFooter
            error={value.error}
            failedAction={value.failedAction}
            hasSaved={value.hasSaved}
            canRetrySave={value.draftText.trim() !== ''}
            isSaving={isSaving}
            isResetting={isResetting}
            onRetrySave={() => send(IdentityEvents.Flush())}
            onRetryReset={() => send(IdentityEvents.Reset())}
          />
        </div>
      )}
    </MachineState>
  )
}

function UnsavedGuard() {
  const machine = IdentityOwner.useMachine()
  const isDirty = AsyncResult.match(useMachineResult(AtomMachine.select(machine, '')), {
    onInitial: () => false,
    onFailure: () => false,
    onSuccess: ({ value }) =>
      Option.match(value, {
        onNone: () => false,
        onSome: root => root.draftText !== root.savedText,
      }),
  })
  const isSaving = AsyncResult.match(useMachineResult(AtomMachine.matches('Saving')(machine)), {
    onInitial: () => false,
    onFailure: () => false,
    onSuccess: ({ value }) => value,
  })
  const shouldBlock = isDirty || isSaving
  const blocker = useBlocker({
    shouldBlockFn: ({ next }) => shouldBlock && next.pathname !== '/login',
    enableBeforeUnload: () => shouldBlock,
    withResolver: true,
  })
  useEffect(() => {
    if (blocker.status === 'blocked' && !shouldBlock) {
      blocker.proceed()
    }
  }, [blocker, shouldBlock])
  if (blocker.status !== 'blocked') return null
  return (
    <AlertDialog open onOpenChange={open => (!open ? blocker.reset() : undefined)}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Unsaved changes</AlertDialogTitle>
          <AlertDialogDescription>
            Your identity edits have not finished saving yet.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel onClick={() => blocker.reset()}>Stay</AlertDialogCancel>
          <AlertDialogCancel variant="destructive" onClick={() => blocker.proceed()}>
            Leave without saving
          </AlertDialogCancel>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}
