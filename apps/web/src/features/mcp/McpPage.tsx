import type { McpServer, RequestPayload } from '@blob-1/contracts'
import { useAtomRefresh, useAtomSet, useAtomValue } from '@effect/atom-react'
import { AtomMachine } from '@typeonce/effect-machine/reactivity'
import { MachineState, useMachineResult } from '@typeonce/effect-machine-react'
import { Option } from 'effect'
import { AsyncResult } from 'effect/reactivity'
import { ExternalLink, Link2, Plus, RefreshCw, Server, Terminal, Trash2 } from 'lucide-react'
import { type FormEvent, type ReactNode, useEffect, useRef, useState } from 'react'

import { ConfirmDialog } from '~/components/ConfirmDialog'
import { EmptyLine, FieldError, FieldRow, Page, PageError, Section } from '~/components/layout/Page'
import { Button } from '~/components/ui/button'
import { Input } from '~/components/ui/input'
import { Switch } from '~/components/ui/switch'
import { ToggleGroup, ToggleGroupItem } from '~/components/ui/toggle-group'
import { Tooltip, TooltipContent, TooltipTrigger } from '~/components/ui/tooltip'
import { MachineAtoms } from '~/lib/runtime'
import { createPersistentMachineContext } from '~/lib/shellMachines'
import { matchCached } from '~/lib/useBackgroundRefresh'
import { cn } from '~/lib/utils'

import { McpEvents, type McpInput, McpMachine } from './mcpMachine'
import { mcpPageAtom } from './mcpState'

const McpOwner = createPersistentMachineContext('mcp', ({ input }: { input: McpInput }) =>
  MachineAtoms.make(McpMachine, input),
)

/** The MCP settings page. */
export function McpPage() {
  const result = useAtomValue(mcpPageAtom)
  const retry = useAtomRefresh(mcpPageAtom)
  return matchCached(result, {
    onInitial: () => (
      <Page title="MCP" wide>
        <EmptyLine>Loading…</EmptyLine>
      </Page>
    ),
    onFailure: failure => (
      <Page title="MCP" wide>
        <PageError cause={failure.cause} onRetry={retry} />
      </Page>
    ),
    onSuccess: value => (
      <McpOwner.Provider input={{ servers: value }}>
        <McpContent />
      </McpOwner.Provider>
    ),
  })
}

function McpProbeWatcher() {
  const machine = McpOwner.useMachine()
  const send = useAtomSet(machine.send)
  return (
    <MachineState machine={machine} path="">
      {({ value }) => {
        const hasProbing = value.servers.some(
          server => server.enabled && (server.status === 'connecting' || server.status === 'idle'),
        )
        return <McpProbePoller active={hasProbing} onRefresh={() => send(McpEvents.Refresh())} />
      }}
    </MachineState>
  )
}

function McpProbePoller(props: { readonly active: boolean; readonly onRefresh: () => void }) {
  const { active, onRefresh } = props
  useEffect(() => {
    if (!active) return
    const timer = window.setInterval(onRefresh, 1500)
    return () => window.clearInterval(timer)
  }, [active, onRefresh])
  return null
}

function McpContent() {
  const machine = McpOwner.useMachine()
  const send = useAtomSet(machine.send)
  useEffect(() => {
    send(McpEvents.Refresh())
  }, [send])
  const addButtonRef = useRef<HTMLButtonElement>(null)
  const isAddClosed = AsyncResult.getOrElse(
    useMachineResult(AtomMachine.matches(machine, 'Add.Closed')),
    () => true,
  )
  const isRefreshing = AsyncResult.getOrElse(
    useMachineResult(AtomMachine.matches(machine, 'Refresh.Working')),
    () => false,
  )
  const wasAddClosed = useRef(isAddClosed)
  useEffect(() => {
    if (!wasAddClosed.current && isAddClosed) {
      addButtonRef.current?.focus()
    }
    wasAddClosed.current = isAddClosed
  }, [isAddClosed])

  return (
    <Page
      title="MCP"
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
                  aria-label="Refresh servers"
                  disabled={isRefreshing}
                  onClick={() => send(McpEvents.Refresh())}
                >
                  <RefreshCw />
                </Button>
              }
            />
            <TooltipContent>{isRefreshing ? 'Refreshing…' : 'Refresh servers'}</TooltipContent>
          </Tooltip>
          <MachineState machine={machine} path="Add.Closed">
            {() => (
              <Button ref={addButtonRef} onClick={() => send(McpEvents.OpenAdd())}>
                <Plus data-icon="inline-start" />
                Add server
              </Button>
            )}
          </MachineState>
        </>
      }
    >
      <MachineState machine={machine} path="Add.Closed" inactive={<AddSection />}>
        {() => null}
      </MachineState>
      <McpProbeWatcher />
      <ServersList />
    </Page>
  )
}

function ServersList() {
  const machine = McpOwner.useMachine()
  return (
    <MachineState machine={machine} path="">
      {({ value, states }) =>
        value.servers.length === 0 ? (
          <EmptyLine>No MCP servers configured yet.</EmptyLine>
        ) : (
          <ul className="divide-y divide-border overflow-hidden rounded-2xl border border-border bg-card shadow-xs">
            {value.servers.map(server => {
              const removeState = states.Remove.state
              const loginState = states.Login.state
              return (
                <ServerRow
                  key={server.id}
                  server={server}
                  removing={removeState.path === 'Remove.Working'}
                  loggingIn={loginState.path === 'Login.Working'}
                  loggingInThis={
                    loginState.path === 'Login.Working' && loginState.value.id === server.id
                  }
                />
              )
            })}
          </ul>
        )
      }
    </MachineState>
  )
}

interface ChipProps {
  readonly className?: string
  readonly children: ReactNode
}

function Chip(props: ChipProps) {
  return (
    <span
      className={cn(
        'inline-flex items-center gap-1.5 rounded-full bg-muted px-2 py-px text-xs font-medium',
        props.className,
      )}
    >
      {props.children}
    </span>
  )
}

function statusBadge(server: McpServer) {
  if (!server.enabled || server.status === 'disconnected') {
    return <Chip className="text-muted-foreground">Disabled</Chip>
  }
  switch (server.status) {
    case 'connected':
      return (
        <Chip className="text-foreground">
          <span aria-hidden="true" className="size-1.5 rounded-full bg-ok" />
          {server.toolCount === undefined
            ? 'Connected'
            : `Connected · ${server.toolCount} ${server.toolCount === 1 ? 'tool' : 'tools'}`}
        </Chip>
      )
    case 'connecting':
      return <Chip className="text-muted-foreground">Connecting…</Chip>
    case 'idle':
      return <Chip className="text-muted-foreground">Standby</Chip>
    case 'needs-login':
      return <Chip className="text-destructive">Needs sign-in</Chip>
    case 'error':
      return <Chip className="text-destructive">Error</Chip>
  }
}

const serverDetail = (server: McpServer): string =>
  server.transport === 'stdio'
    ? [server.command, ...(server.args ?? [])].join(' ')
    : (server.url ?? '')

const needsOauth = (server: McpServer): boolean =>
  server.enabled &&
  server.transport === 'http' &&
  (server.status === 'needs-login' || (server.auth === 'oauth' && server.status !== 'connected'))

interface ServerRowProps {
  readonly server: McpServer
  readonly removing: boolean
  readonly loggingIn: boolean
  readonly loggingInThis: boolean
}

function ServerRow(props: ServerRowProps) {
  const machine = McpOwner.useMachine()
  const send = useAtomSet(machine.send)
  const { server, removing, loggingIn, loggingInThis } = props
  return (
    <li className="flex flex-col gap-3 p-4 sm:flex-row sm:items-center sm:gap-4">
      <span
        aria-hidden="true"
        className="hidden size-10 shrink-0 place-items-center rounded-full border border-border bg-shelf text-muted-foreground sm:grid"
      >
        <Server className="size-5" />
      </span>
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-center gap-2">
          <p className="min-w-0 break-words text-sm font-medium text-foreground [overflow-wrap:anywhere]">
            {server.id}
          </p>
          {statusBadge(server)}
        </div>
        <div className="mt-1.5 flex min-w-0 items-start gap-1.5 text-muted-foreground sm:mt-1">
          <span className="mt-0.5 shrink-0">
            {server.transport === 'stdio' ? (
              <Terminal className="size-3.5" />
            ) : (
              <Link2 className="size-3.5" />
            )}
          </span>
          <span className="min-w-0 break-words font-mono text-xs select-text [overflow-wrap:anywhere]">
            {serverDetail(server)}
          </span>
        </div>
        {server.error === undefined ? null : (
          <p
            role="alert"
            className="mt-1.5 break-words text-xs text-destructive [overflow-wrap:anywhere]"
          >
            {server.error}
          </p>
        )}
      </div>
      <div className="flex shrink-0 items-center justify-end gap-3">
        {needsOauth(server) ? (
          <Button
            variant="outline"
            disabled={loggingIn}
            onClick={() => send(McpEvents.Login({ id: server.id }))}
          >
            {loggingInThis ? 'Signing in…' : 'Sign in'}
            {loggingInThis ? null : <ExternalLink data-icon="inline-end" className="size-3.5" />}
          </Button>
        ) : null}
        <Switch
          aria-label={`Enable ${server.id}`}
          checked={server.enabled}
          onCheckedChange={enabled => send(McpEvents.SetEnabled({ id: server.id, enabled }))}
        />
        <ConfirmDialog
          tooltip="Remove server"
          trigger={
            <Button
              variant="ghost"
              size="icon"
              aria-label={`Remove ${server.id}`}
              className="text-destructive hover:bg-destructive/10 hover:text-destructive"
              disabled={removing}
            >
              <Trash2 />
            </Button>
          }
          title="Remove MCP server?"
          detail={server.id}
          confirmLabel="Remove"
          onConfirm={() => send(McpEvents.RemoveServer({ id: server.id }))}
        />
      </div>
    </li>
  )
}

type AddKind = McpServer['transport']
type AddField = 'id' | 'command' | 'env' | 'url' | 'headers'
type FieldErrors = Partial<Record<AddField, string>>

const addKinds: ReadonlyArray<{ readonly kind: AddKind; readonly label: string }> = [
  { kind: 'stdio', label: 'Stdio' },
  { kind: 'http', label: 'HTTP' },
]

interface AddDraft {
  readonly kind: AddKind
  readonly id: string
  readonly command: string
  readonly args: string
  readonly env: string
  readonly url: string
  readonly headers: string
}

const fieldElementIds = {
  id: 'mcp-add-id',
  command: 'mcp-add-command',
  env: 'mcp-add-env',
  url: 'mcp-add-url',
  headers: 'mcp-add-headers',
} satisfies Record<AddField, string>

const MCP_ID = /^[a-z0-9-]+$/

// Env entries are whitespace-separated `KEY=VALUE`; header entries are
// `;`-separated `Name: ENV_VAR`, matching the CLI flag shapes.
const parseEnv = (input: string): Record<string, string> | null => {
  const env: Record<string, string> = {}
  for (const entry of input.trim().split(/\s+/)) {
    if (entry === '') continue
    const index = entry.indexOf('=')
    if (index < 1) return null
    env[entry.slice(0, index)] = entry.slice(index + 1)
  }
  return env
}

const parseHeaders = (input: string): Record<string, string> | null => {
  const headers: Record<string, string> = {}
  for (const entry of input.split(';')) {
    const trimmed = entry.trim()
    if (trimmed === '') continue
    const index = trimmed.indexOf(':')
    if (index < 1) return null
    headers[trimmed.slice(0, index).trim()] = trimmed.slice(index + 1).trim()
  }
  return headers
}

const validateDraft = (draft: AddDraft, existingIds: ReadonlyArray<string>) => {
  const errors: Partial<Record<AddField, string>> = {}
  const id = draft.id.trim()
  if (!MCP_ID.test(id)) {
    errors.id = 'Use lowercase letters, digits, and -.'
  } else if (existingIds.includes(id)) {
    errors.id = 'A server with this ID already exists.'
  }
  if (draft.kind === 'stdio') {
    if (draft.command.trim() === '') {
      errors.command = 'Required'
    }
    if (parseEnv(draft.env) === null) {
      errors.env = 'Use KEY=VALUE pairs separated by spaces.'
    }
  } else {
    if (!URL.canParse(draft.url.trim())) {
      errors.url = 'Enter a valid URL.'
    }
    if (parseHeaders(draft.headers) === null) {
      errors.headers = 'Use Name: ENV_VAR entries separated by ;.'
    }
  }
  const order: ReadonlyArray<AddField> =
    draft.kind === 'stdio' ? ['id', 'command', 'env'] : ['id', 'url', 'headers']
  const firstField = order.find(field => errors[field] !== undefined) ?? null
  return { errors, firstField }
}

const buildPayload = (draft: AddDraft): RequestPayload<'mcp.add'> => {
  const id = draft.id.trim()
  if (draft.kind === 'stdio') {
    return {
      id,
      transport: 'stdio',
      command: draft.command.trim(),
      args: draft.args
        .trim()
        .split(/\s+/)
        .filter(arg => arg !== ''),
      env: parseEnv(draft.env) ?? {},
    }
  }
  return {
    id,
    transport: 'http',
    url: draft.url.trim(),
    headers: parseHeaders(draft.headers) ?? {},
  }
}

function AddSection() {
  const machine = McpOwner.useMachine()
  return (
    <Section title="New server" className="mb-8 max-w-[720px]">
      <MachineState machine={machine} path="">
        {({ value }) => <AddForm existingIds={value.servers.map(server => server.id)} />}
      </MachineState>
    </Section>
  )
}

const initialDraft: AddDraft = {
  kind: 'stdio',
  id: '',
  command: '',
  args: '',
  env: '',
  url: '',
  headers: '',
}

interface AddFormProps {
  readonly existingIds: ReadonlyArray<string>
}

interface TransportFieldsProps {
  readonly draft: AddDraft
  readonly errors: FieldErrors
  readonly disabled: boolean
  readonly onEditValidated: (field: AddField, value: string) => void
  readonly onEditArgs: (value: string) => void
}

function StdioFields(props: TransportFieldsProps) {
  return (
    <>
      <FieldRow id="mcp-add-command" label="Command">
        <Input
          id="mcp-add-command"
          className="font-mono"
          placeholder="npx"
          value={props.draft.command}
          disabled={props.disabled}
          aria-invalid={props.errors.command !== undefined}
          aria-describedby={
            props.errors.command === undefined ? undefined : 'mcp-add-command-error'
          }
          onChange={event => props.onEditValidated('command', event.target.value)}
        />
        {props.errors.command === undefined ? null : (
          <FieldError id="mcp-add-command-error">{props.errors.command}</FieldError>
        )}
      </FieldRow>
      <FieldRow id="mcp-add-args" label="Arguments">
        <Input
          id="mcp-add-args"
          className="font-mono"
          placeholder="-y @modelcontextprotocol/server-filesystem /tmp"
          value={props.draft.args}
          disabled={props.disabled}
          onChange={event => props.onEditArgs(event.target.value)}
        />
      </FieldRow>
      <FieldRow id="mcp-add-env" label="Environment">
        <Input
          id="mcp-add-env"
          className="font-mono"
          placeholder="KEY=VALUE OTHER=VALUE"
          value={props.draft.env}
          disabled={props.disabled}
          aria-invalid={props.errors.env !== undefined}
          aria-describedby={props.errors.env === undefined ? undefined : 'mcp-add-env-error'}
          onChange={event => props.onEditValidated('env', event.target.value)}
        />
        {props.errors.env === undefined ? null : (
          <FieldError id="mcp-add-env-error">{props.errors.env}</FieldError>
        )}
      </FieldRow>
    </>
  )
}

function HttpFields(props: Omit<TransportFieldsProps, 'onEditArgs'>) {
  return (
    <>
      <FieldRow id="mcp-add-url" label="URL">
        <Input
          id="mcp-add-url"
          className="font-mono"
          placeholder="https://mcp.example.com/mcp"
          value={props.draft.url}
          disabled={props.disabled}
          aria-invalid={props.errors.url !== undefined}
          aria-describedby={props.errors.url === undefined ? undefined : 'mcp-add-url-error'}
          onChange={event => props.onEditValidated('url', event.target.value)}
        />
        {props.errors.url === undefined ? null : (
          <FieldError id="mcp-add-url-error">{props.errors.url}</FieldError>
        )}
      </FieldRow>
      <FieldRow id="mcp-add-headers" label="Headers">
        <Input
          id="mcp-add-headers"
          className="font-mono"
          placeholder="Authorization: API_TOKEN; X-Team: TEAM_ID"
          value={props.draft.headers}
          disabled={props.disabled}
          aria-invalid={props.errors.headers !== undefined}
          aria-describedby={
            props.errors.headers === undefined ? undefined : 'mcp-add-headers-error'
          }
          onChange={event => props.onEditValidated('headers', event.target.value)}
        />
        {props.errors.headers === undefined ? null : (
          <FieldError id="mcp-add-headers-error">{props.errors.headers}</FieldError>
        )}
      </FieldRow>
    </>
  )
}

function AddForm(props: AddFormProps) {
  const machine = McpOwner.useMachine()
  const send = useAtomSet(machine.send)
  const idRef = useRef<HTMLInputElement>(null)
  useEffect(() => {
    idRef.current?.focus()
  }, [])
  const [draft, setDraft] = useState<AddDraft>(initialDraft)
  const [errors, setErrors] = useState<FieldErrors>({})
  const isSubmitting = AsyncResult.getOrElse(
    useMachineResult(AtomMachine.matches(machine, 'Add.Submitting')),
    () => false,
  )
  const serverError = Option.getOrNull(
    AsyncResult.getOrElse(useMachineResult(AtomMachine.select(machine, 'Add.Editing')), () =>
      Option.none(),
    ),
  )?.error

  const clearServerError = () => {
    if (serverError != null) send(McpEvents.ClearAddError())
  }

  const editValidatedField = (field: AddField, value: string) => {
    setDraft(current => ({ ...current, [field]: value }))
    setErrors(current => {
      if (current[field] === undefined) return current
      const next = { ...current }
      delete next[field]
      return next
    })
    clearServerError()
  }

  const submit = (event: FormEvent) => {
    event.preventDefault()
    const validation = validateDraft(draft, props.existingIds)
    setErrors(validation.errors)
    if (validation.firstField !== null) {
      document.getElementById(fieldElementIds[validation.firstField])?.focus()
      return
    }
    send(McpEvents.SubmitAdd({ payload: buildPayload(draft) }))
  }

  return (
    <form noValidate onSubmit={submit}>
      <FieldRow labelId="mcp-add-kind-label" label="Transport">
        <ToggleGroup
          aria-labelledby="mcp-add-kind-label"
          spacing={1}
          disabled={isSubmitting}
          value={[draft.kind]}
          onValueChange={values => {
            const next = addKinds.find(option => option.kind === values[0])?.kind
            if (next === undefined) return
            setDraft(current => ({ ...current, kind: next }))
            setErrors(current => (current.id === undefined ? {} : { id: current.id }))
            clearServerError()
          }}
          className="h-10 rounded-full bg-muted p-[3px] md:h-8"
        >
          {addKinds.map(option => (
            <ToggleGroupItem
              key={option.kind}
              value={option.kind}
              className="h-full rounded-full px-3 text-muted-foreground hover:bg-transparent hover:text-foreground aria-pressed:bg-background aria-pressed:text-foreground aria-pressed:shadow-sm"
            >
              {option.label}
            </ToggleGroupItem>
          ))}
        </ToggleGroup>
      </FieldRow>
      <FieldRow id="mcp-add-id" label="ID">
        <Input
          ref={idRef}
          id="mcp-add-id"
          className="font-mono"
          placeholder="filesystem"
          value={draft.id}
          disabled={isSubmitting}
          aria-invalid={errors.id !== undefined}
          aria-describedby={errors.id === undefined ? undefined : 'mcp-add-id-error'}
          onChange={event => editValidatedField('id', event.target.value)}
        />
        {errors.id === undefined ? null : (
          <FieldError id="mcp-add-id-error">{errors.id}</FieldError>
        )}
      </FieldRow>
      {draft.kind === 'stdio' ? (
        <StdioFields
          draft={draft}
          errors={errors}
          disabled={isSubmitting}
          onEditValidated={editValidatedField}
          onEditArgs={value => {
            setDraft(current => ({ ...current, args: value }))
            clearServerError()
          }}
        />
      ) : (
        <HttpFields
          draft={draft}
          errors={errors}
          disabled={isSubmitting}
          onEditValidated={editValidatedField}
        />
      )}
      <div className="mt-3 flex flex-col items-start gap-2 md:pl-[120px]">
        <AddButtons />
        <AddError />
      </div>
    </form>
  )
}

function AddButtons() {
  const machine = McpOwner.useMachine()
  const send = useAtomSet(machine.send)
  return (
    <div className="flex items-center gap-2">
      <MachineState
        machine={machine}
        path="Add.Submitting"
        inactive={
          <>
            <Button type="submit">Add server</Button>
            <Button type="button" variant="secondary" onClick={() => send(McpEvents.CloseAdd())}>
              Cancel
            </Button>
          </>
        }
      >
        {() => (
          <>
            <Button type="submit" disabled>
              Adding server…
            </Button>
            <Button type="button" variant="secondary" disabled>
              Cancel
            </Button>
          </>
        )}
      </MachineState>
    </div>
  )
}

function AddError() {
  const machine = McpOwner.useMachine()
  return (
    <MachineState machine={machine} path="Add.Editing">
      {({ value }) =>
        value.error === null ? null : (
          <p role="alert" className="break-words text-sm text-destructive [overflow-wrap:anywhere]">
            {value.error}
          </p>
        )
      }
    </MachineState>
  )
}
