import type { EnvironmentInfo, PendingNode } from '@blob-1/contracts'
import { useAtomRefresh, useAtomSet, useAtomValue } from '@effect/atom-react'
import { AtomMachine } from '@typeonce/effect-machine/reactivity'
import { MachineState, useMachineResult } from '@typeonce/effect-machine-react'
import { AsyncResult } from 'effect/reactivity'
import { Monitor, Pencil, RefreshCw, Server, Trash2 } from 'lucide-react'
import { type FormEvent, useEffect, useState } from 'react'

import { ConfirmDialog } from '~/components/ConfirmDialog'
import { EmptyLine, FieldRow, Page, PageError, Section } from '~/components/layout/Page'
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

import {
  EnvironmentsEvents,
  type EnvironmentsInput,
  EnvironmentsMachine,
} from './environmentsMachine'
import { environmentsPageAtom } from './environmentsState'

const EnvironmentsOwner = createPersistentMachineContext(
  'environments',
  ({ input }: { readonly input: EnvironmentsInput }) =>
    MachineAtoms.make(EnvironmentsMachine, input),
)

function formatTimestamp(timestampSecs: number): string {
  const deltaSecs = Math.max(0, Math.floor(Date.now() / 1000) - timestampSecs)
  if (deltaSecs < 60) return 'Just now'
  if (deltaSecs < 3600) return `${Math.floor(deltaSecs / 60)}m ago`
  if (deltaSecs < 86400) return `${Math.floor(deltaSecs / 3600)}h ago`
  return new Date(timestampSecs * 1000).toLocaleDateString(undefined, {
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  })
}

/** The remote execution environments settings page. */
export function EnvironmentsPage() {
  const result = useAtomValue(environmentsPageAtom)
  const retry = useAtomRefresh(environmentsPageAtom)
  return matchCached(result, {
    onInitial: () => (
      <Page title="Environments" wide>
        <EmptyLine>Loading…</EmptyLine>
      </Page>
    ),
    onFailure: failure => (
      <Page title="Environments" wide>
        <PageError cause={failure.cause} onRetry={retry} />
      </Page>
    ),
    onSuccess: value => (
      <EnvironmentsOwner.Provider input={value}>
        <EnvironmentsContent />
      </EnvironmentsOwner.Provider>
    ),
  })
}

function EnvironmentsContent() {
  const machine = EnvironmentsOwner.useMachine()
  const send = useAtomSet(machine.send)
  useEffect(() => {
    send(EnvironmentsEvents.Refresh())
  }, [send])
  const isRefreshing = AsyncResult.getOrElse(
    useMachineResult(AtomMachine.matches(machine, 'Sync.Refreshing')),
    () => false,
  )

  return (
    <Page
      title="Environments"
      wide
      actions={
        <>
          <span aria-live="polite" className="text-xs text-muted-foreground empty:sr-only">
            {isRefreshing ? 'Refreshing…' : null}
          </span>
          <Tooltip>
            <TooltipTrigger
              render={
                <Button
                  variant="outline"
                  size="icon"
                  aria-label="Refresh environments"
                  disabled={isRefreshing}
                  onClick={() => send(EnvironmentsEvents.Refresh())}
                >
                  <RefreshCw />
                </Button>
              }
            />
            <TooltipContent>{isRefreshing ? 'Refreshing…' : 'Refresh environments'}</TooltipContent>
          </Tooltip>
        </>
      }
    >
      <MachineState machine={machine} path="">
        {({ value, states }) => {
          const busy = states.Action.state.path !== 'Action.Idle'
          return (
            <div className="space-y-8">
              <Section title="Pending">
                <PendingList pending={value.pending} busy={busy} />
              </Section>
              <Section title="Environments">
                <EnvironmentList environments={value.environments} busy={busy} />
              </Section>
            </div>
          )
        }}
      </MachineState>
    </Page>
  )
}

interface PendingListProps {
  readonly pending: ReadonlyArray<PendingNode>
  readonly busy: boolean
}

function PendingList(props: PendingListProps) {
  if (props.pending.length === 0) {
    return <EmptyLine>No nodes waiting for approval.</EmptyLine>
  }
  return (
    <ul className="divide-y divide-border overflow-hidden rounded-2xl border border-border bg-card shadow-xs">
      {props.pending.map(node => (
        <PendingRow key={node.nodeId} node={node} busy={props.busy} />
      ))}
    </ul>
  )
}

interface PendingRowProps {
  readonly node: PendingNode
  readonly busy: boolean
}

function PendingRow(props: PendingRowProps) {
  const [open, setOpen] = useState(false)
  const { node } = props
  return (
    <li className="flex flex-col gap-3 p-4 sm:flex-row sm:items-center sm:gap-4">
      <span
        aria-hidden="true"
        className="hidden size-10 shrink-0 place-items-center rounded-full border border-border bg-shelf text-muted-foreground sm:grid"
      >
        <Monitor className="size-5" />
      </span>
      <div className="min-w-0 flex-1 space-y-1">
        <p className="min-w-0 break-words text-sm font-medium text-foreground [overflow-wrap:anywhere]">
          {node.hostname}
        </p>
        <p className="text-xs text-muted-foreground tabular-nums">
          v{node.version} · Connected {formatTimestamp(node.connectedAt)}
        </p>
      </div>
      <div className="flex shrink-0 items-center justify-end gap-3">
        <Button size="sm" disabled={props.busy} onClick={() => setOpen(true)}>
          Accept
        </Button>
      </div>
      <AcceptDialog node={node} open={open} onOpenChange={setOpen} />
    </li>
  )
}

interface AcceptDialogProps {
  readonly node: PendingNode
  readonly open: boolean
  readonly onOpenChange: (open: boolean) => void
}

function AcceptDialog(props: AcceptDialogProps) {
  return (
    <Dialog open={props.open} onOpenChange={props.onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Accept node</DialogTitle>
        </DialogHeader>
        {props.open ? (
          <AcceptForm node={props.node} onClose={() => props.onOpenChange(false)} />
        ) : null}
      </DialogContent>
    </Dialog>
  )
}

interface AcceptFormProps {
  readonly node: PendingNode
  readonly onClose: () => void
}

function AcceptForm(props: AcceptFormProps) {
  const machine = EnvironmentsOwner.useMachine()
  const send = useAtomSet(machine.send)
  const [name, setName] = useState('')

  const handleSubmit = (event: FormEvent) => {
    event.preventDefault()
    send(EnvironmentsEvents.Accept({ nodeId: props.node.nodeId, name: name.trim() }))
    props.onClose()
  }

  return (
    <form onSubmit={handleSubmit} className="space-y-2">
      <FieldRow id="accept-node-name" label="Name">
        <Input
          id="accept-node-name"
          placeholder={props.node.hostname}
          value={name}
          onChange={event => setName(event.target.value)}
        />
      </FieldRow>
      <DialogFooter className="pt-2">
        <Button type="button" variant="ghost" onClick={props.onClose}>
          Cancel
        </Button>
        <Button type="submit">Accept</Button>
      </DialogFooter>
    </form>
  )
}

interface EnvironmentListProps {
  readonly environments: ReadonlyArray<EnvironmentInfo>
  readonly busy: boolean
}

function EnvironmentList(props: EnvironmentListProps) {
  if (props.environments.length === 0) {
    return <EmptyLine>No environments yet.</EmptyLine>
  }
  return (
    <ul className="divide-y divide-border overflow-hidden rounded-2xl border border-border bg-card shadow-xs">
      {props.environments.map(environment => (
        <EnvironmentRow key={environment.name} environment={environment} busy={props.busy} />
      ))}
    </ul>
  )
}

interface EnvironmentRowProps {
  readonly environment: EnvironmentInfo
  readonly busy: boolean
}

function EnvironmentRow(props: EnvironmentRowProps) {
  const machine = EnvironmentsOwner.useMachine()
  const send = useAtomSet(machine.send)
  const [renameOpen, setRenameOpen] = useState(false)
  const { environment } = props

  return (
    <li className="flex flex-col gap-3 p-4 sm:flex-row sm:items-center sm:gap-4">
      <span
        aria-hidden="true"
        className="hidden size-10 shrink-0 place-items-center rounded-full border border-border bg-shelf text-muted-foreground sm:grid"
      >
        <Server className="size-5" />
      </span>
      <div className="min-w-0 flex-1 space-y-1">
        <div className="flex flex-wrap items-center gap-2">
          <p className="min-w-0 break-words text-sm font-medium text-foreground [overflow-wrap:anywhere]">
            {environment.name}
          </p>
          <span className="inline-flex items-center gap-1.5 rounded-full bg-muted px-2 py-px text-xs font-medium text-foreground">
            <span
              aria-hidden="true"
              className={
                environment.online
                  ? 'size-1.5 rounded-full bg-ok'
                  : 'size-1.5 rounded-full bg-muted-foreground/40'
              }
            />
            {environment.online ? 'Online' : 'Offline'}
          </span>
        </div>
        <p className="text-xs text-muted-foreground tabular-nums">
          {environment.version === null ? 'Version unknown' : `v${environment.version}`} · Last seen{' '}
          {formatTimestamp(environment.lastSeen)}
        </p>
      </div>
      <div className="flex shrink-0 items-center justify-end gap-3">
        <Tooltip>
          <TooltipTrigger
            render={
              <Button
                variant="ghost"
                size="icon"
                aria-label={`Rename ${environment.name}`}
                disabled={props.busy}
                onClick={() => setRenameOpen(true)}
              >
                <Pencil />
              </Button>
            }
          />
          <TooltipContent>Rename</TooltipContent>
        </Tooltip>
        <ConfirmDialog
          tooltip="Revoke environment"
          trigger={
            <Button
              variant="ghost"
              size="icon"
              aria-label={`Revoke ${environment.name}`}
              className="text-destructive hover:bg-destructive/10 hover:text-destructive"
              disabled={props.busy}
            >
              <Trash2 />
            </Button>
          }
          title={`Revoke ${environment.name}?`}
          detail="The node drops back to pending and bots bound to it pause."
          confirmLabel="Revoke"
          onConfirm={() => send(EnvironmentsEvents.Remove({ name: environment.name }))}
        />
      </div>
      <RenameDialog environment={environment} open={renameOpen} onOpenChange={setRenameOpen} />
    </li>
  )
}

interface RenameDialogProps {
  readonly environment: EnvironmentInfo
  readonly open: boolean
  readonly onOpenChange: (open: boolean) => void
}

function RenameDialog(props: RenameDialogProps) {
  return (
    <Dialog open={props.open} onOpenChange={props.onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Rename environment</DialogTitle>
        </DialogHeader>
        {props.open ? (
          <RenameForm environment={props.environment} onClose={() => props.onOpenChange(false)} />
        ) : null}
      </DialogContent>
    </Dialog>
  )
}

interface RenameFormProps {
  readonly environment: EnvironmentInfo
  readonly onClose: () => void
}

function RenameForm(props: RenameFormProps) {
  const machine = EnvironmentsOwner.useMachine()
  const send = useAtomSet(machine.send)
  const [name, setName] = useState(props.environment.name)

  const handleSubmit = (event: FormEvent) => {
    event.preventDefault()
    const next = name.trim()
    if (next === '' || next === props.environment.name) return
    send(EnvironmentsEvents.Rename({ name: props.environment.name, to: next }))
    props.onClose()
  }

  return (
    <form onSubmit={handleSubmit} className="space-y-2">
      <FieldRow id="rename-environment-name" label="Name">
        <Input
          id="rename-environment-name"
          value={name}
          onChange={event => setName(event.target.value)}
        />
      </FieldRow>
      <DialogFooter className="pt-2">
        <Button type="button" variant="ghost" onClick={props.onClose}>
          Cancel
        </Button>
        <Button
          type="submit"
          disabled={name.trim() === '' || name.trim() === props.environment.name}
        >
          Rename
        </Button>
      </DialogFooter>
    </form>
  )
}
