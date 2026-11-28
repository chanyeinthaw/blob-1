import type { BrowserProfile, BrowserState } from '@blob-1/contracts'
import { useAtomRefresh, useAtomSet, useAtomValue } from '@effect/atom-react'
import { MachineState } from '@typeonce/effect-machine-react'
import { Plus, Trash2 } from 'lucide-react'
import { type FormEvent, useEffect, useState } from 'react'

import { ConfirmDialog } from '~/components/ConfirmDialog'
import {
  Empty,
  EmptyLine,
  FieldError,
  FieldRow,
  Page,
  PageError,
  tableRowHoverClassName,
} from '~/components/layout/Page'
import { Button } from '~/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '~/components/ui/dialog'
import { Input } from '~/components/ui/input'
import { Tooltip, TooltipContent, TooltipTrigger } from '~/components/ui/tooltip'
import { MachineAtoms } from '~/lib/runtime'
import { createPersistentMachineContext } from '~/lib/shellMachines'
import { matchCached } from '~/lib/useBackgroundRefresh'
import { cn } from '~/lib/utils'

import { BrowserEvents, type BrowserInput, BrowserMachine } from './browserMachine'
import { browserPageAtom } from './browserState'

const BrowserOwner = createPersistentMachineContext(
  'browser',
  ({ input }: { readonly input: BrowserInput }) => MachineAtoms.make(BrowserMachine, input),
)

/** Profile | Bots | actions, shared by the header and every row. */
const profileGridClassName = 'sm:grid-cols-[minmax(0,0.8fr)_minmax(0,1.2fr)_48px]'

const rowClassName =
  'group grid grid-cols-1 items-center gap-x-3 gap-y-2.5 px-1 py-3 sm:min-h-12 sm:gap-y-1.5 sm:px-2 sm:py-2 ' +
  tableRowHoverClassName

/** The global browser settings page. */
export function BrowserPage() {
  const result = useAtomValue(browserPageAtom)
  const retry = useAtomRefresh(browserPageAtom)
  return matchCached(result, {
    onInitial: () => (
      <Page title="Browser" width="wide">
        <EmptyLine>Loading…</EmptyLine>
      </Page>
    ),
    onFailure: failure => (
      <Page title="Browser" width="wide">
        <PageError cause={failure.cause} onRetry={retry} />
      </Page>
    ),
    onSuccess: value => (
      <BrowserOwner.Provider input={value}>
        <BrowserContent />
      </BrowserOwner.Provider>
    ),
  })
}

function BrowserContent() {
  const machine = BrowserOwner.useMachine()
  const send = useAtomSet(machine.send)
  useEffect(() => {
    send(BrowserEvents.Refresh())
  }, [send])

  return (
    <Page
      title="Browser"
      width="wide"
      actions={
        <>
          <BrowserStatusPill />
          <Button onClick={() => send(BrowserEvents.OpenAdd())}>
            <Plus data-icon="inline-start" />
            Add profile
          </Button>
        </>
      }
    >
      <StartingPoller />
      <AddProfileDialog />
      <ProfileList />
    </Page>
  )
}

function StartingPoller() {
  const machine = BrowserOwner.useMachine()
  const send = useAtomSet(machine.send)
  return (
    <MachineState machine={machine} path="">
      {({ value }) => (
        <PollWhileStarting
          active={value.data.state === 'starting'}
          onRefresh={() => send(BrowserEvents.Refresh())}
        />
      )}
    </MachineState>
  )
}

function PollWhileStarting(props: { readonly active: boolean; readonly onRefresh: () => void }) {
  const { active, onRefresh } = props
  useEffect(() => {
    if (!active) return
    const timer = window.setInterval(onRefresh, 1500)
    return () => window.clearInterval(timer)
  }, [active, onRefresh])
  return null
}

function BrowserStatusPill() {
  const machine = BrowserOwner.useMachine()
  return (
    <MachineState machine={machine} path="">
      {({ value }) => <StatusPill state={value.data.state} message={value.data.message} />}
    </MachineState>
  )
}

interface StatusPillProps {
  readonly state: BrowserState
  readonly message: string | null
}

function StatusPill(props: StatusPillProps) {
  const label =
    props.state === 'ready'
      ? 'Ready'
      : props.state === 'starting'
        ? 'Starting'
        : props.state === 'error'
          ? 'Error'
          : 'Unavailable'
  const dot =
    props.state === 'ready'
      ? 'bg-ok'
      : props.state === 'starting'
        ? 'bg-amber-500'
        : props.state === 'error'
          ? 'bg-destructive'
          : 'bg-muted-foreground/40'
  const className = cn(
    'inline-flex cursor-default items-center gap-1.5 rounded-full bg-muted px-2.5 py-1 text-xs font-medium whitespace-nowrap outline-none focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring/40',
    props.state === 'error'
      ? 'text-destructive'
      : props.state === 'unavailable'
        ? 'text-muted-foreground'
        : 'text-foreground',
  )
  const content = (
    <>
      <span aria-hidden="true" className={cn('size-1.5 rounded-full', dot)} />
      {label}
    </>
  )
  if (props.state !== 'error' || props.message === null) {
    return <span className={className}>{content}</span>
  }
  return (
    <Tooltip>
      <TooltipTrigger
        render={
          <button
            type="button"
            aria-label={`Browser error: ${props.message}`}
            className={className}
          />
        }
      >
        {content}
      </TooltipTrigger>
      <TooltipContent>{props.message}</TooltipContent>
    </Tooltip>
  )
}

function ProfileList() {
  const machine = BrowserOwner.useMachine()
  return (
    <MachineState machine={machine} path="">
      {({ value, states }) => {
        const busy =
          states.Remove.state.path === 'Remove.Working' ||
          states.Add.state.path === 'Add.Submitting'
        if (value.data.profiles.length === 0) {
          return <Empty>No browser profiles yet.</Empty>
        }
        return (
          <div>
            <ProfilesHeader />
            <ul className="flex flex-col divide-y">
              {value.data.profiles.map(profile => (
                <ProfileRow key={profile.name} profile={profile} busy={busy} />
              ))}
            </ul>
          </div>
        )
      }}
    </MachineState>
  )
}

function ProfilesHeader() {
  return (
    <div
      className={cn(
        'hidden h-10 items-center gap-x-3 border-b border-border px-2 text-sm font-medium text-muted-foreground sm:grid',
        profileGridClassName,
      )}
    >
      <span>Profile</span>
      <span>Bots</span>
      <span />
    </div>
  )
}

interface ProfileRowProps {
  readonly profile: BrowserProfile
  readonly busy: boolean
}

function ProfileRow(props: ProfileRowProps) {
  const machine = BrowserOwner.useMachine()
  const send = useAtomSet(machine.send)
  const { profile } = props
  return (
    <li className={cn(rowClassName, profileGridClassName)}>
      <div className="flex min-w-0 items-center">
        <span className="min-w-0 break-words font-mono text-sm font-medium text-foreground [overflow-wrap:anywhere] sm:truncate">
          {profile.name}
        </span>
      </div>
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2 sm:contents">
        <span className="min-w-0 break-words text-sm text-muted-foreground [overflow-wrap:anywhere] sm:truncate">
          {profile.bots.length === 0 ? '—' : profile.bots.join(', ')}
        </span>
        <div className="flex shrink-0 items-center justify-end">
          <ConfirmDialog
            tooltip="Remove profile"
            trigger={
              <Button
                variant="ghost"
                size="icon-xs"
                aria-label={`Remove ${profile.name}`}
                className="text-muted-foreground hover:bg-destructive/10 hover:text-destructive"
                disabled={props.busy}
              >
                <Trash2 />
              </Button>
            }
            title={`Remove ${profile.name}?`}
            detail="Its cookies and logins are deleted."
            confirmLabel="Remove"
            onConfirm={() => send(BrowserEvents.RemoveProfile({ name: profile.name }))}
          />
        </div>
      </div>
    </li>
  )
}

function AddProfileDialog() {
  const machine = BrowserOwner.useMachine()
  const send = useAtomSet(machine.send)
  return (
    <MachineState machine={machine} path="">
      {({ states }) => {
        const state = states.Add.state
        const open = state.path !== 'Add.Closed'
        const submitting = state.path === 'Add.Submitting'
        return (
          <Dialog
            open={open}
            onOpenChange={next => {
              if (!next && !submitting) send(BrowserEvents.CloseAdd())
            }}
          >
            <DialogContent>
              <DialogHeader>
                <DialogTitle>Add browser profile</DialogTitle>
              </DialogHeader>
              {open ? (
                <AddProfileForm
                  error={state.path === 'Add.Editing' ? state.value.error : null}
                  submitting={submitting}
                />
              ) : null}
            </DialogContent>
          </Dialog>
        )
      }}
    </MachineState>
  )
}

interface AddProfileFormProps {
  readonly error: string | null
  readonly submitting: boolean
}

function AddProfileForm(props: AddProfileFormProps) {
  const machine = BrowserOwner.useMachine()
  const send = useAtomSet(machine.send)
  const [name, setName] = useState('')

  const submit = (event: FormEvent) => {
    event.preventDefault()
    send(BrowserEvents.SubmitAdd({ name }))
  }

  return (
    <form noValidate onSubmit={submit} className="space-y-2">
      <FieldRow id="browser-profile-name" label="Name">
        <Input
          id="browser-profile-name"
          className="font-mono"
          value={name}
          disabled={props.submitting}
          aria-invalid={props.error !== null}
          aria-describedby={props.error === null ? undefined : 'browser-profile-name-error'}
          onChange={event => {
            setName(event.target.value)
            if (props.error !== null) send(BrowserEvents.ClearAddError())
          }}
        />
        {props.error === null ? null : (
          <FieldError id="browser-profile-name-error">{props.error}</FieldError>
        )}
      </FieldRow>
      <DialogFooter className="pt-2">
        <Button
          type="button"
          variant="ghost"
          disabled={props.submitting}
          onClick={() => send(BrowserEvents.CloseAdd())}
        >
          Cancel
        </Button>
        <Button type="submit" disabled={props.submitting}>
          {props.submitting ? 'Adding…' : 'Add profile'}
        </Button>
      </DialogFooter>
    </form>
  )
}
