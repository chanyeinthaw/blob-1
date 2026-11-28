import type { Provider, ProviderApi } from '@blob-1/contracts'
import { useAtomRefresh, useAtomSet, useAtomValue } from '@effect/atom-react'
import { AtomMachine } from '@typeonce/effect-machine/reactivity'
import { MachineState, useMachineResult } from '@typeonce/effect-machine-react'
import { Option } from 'effect'
import { AsyncResult } from 'effect/reactivity'
import { KeyRound, Link2, Pencil, Plus, RefreshCw, Trash2 } from 'lucide-react'
import { type FormEvent, useEffect, useRef, useState } from 'react'

import { ConfirmDialog } from '~/components/ConfirmDialog'
import { EmptyLine, FieldError, FieldRow, Page, PageError, Section } from '~/components/layout/Page'
import { Button } from '~/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '~/components/ui/dialog'
import { Input } from '~/components/ui/input'
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '~/components/ui/select'
import { Tooltip, TooltipContent, TooltipTrigger } from '~/components/ui/tooltip'
import { MachineAtoms } from '~/lib/runtime'
import { createPersistentMachineContext } from '~/lib/shellMachines'
import { matchCached } from '~/lib/useBackgroundRefresh'
import { cn } from '~/lib/utils'

import { ProvidersEvents, type ProvidersInput, ProvidersMachine } from './providersMachine'
import { providersPageAtom } from './providersState'

const ProvidersOwner = createPersistentMachineContext(
  'providers',
  ({ input }: { input: ProvidersInput }) => MachineAtoms.make(ProvidersMachine, input),
)

const apiLabels = {
  responses: 'Responses',
  completions: 'Chat Completions',
  'opencode-go': 'OpenCode Go',
  decision: 'Decision (Clef / Jev)',
}

const apiOptions = [
  { value: 'responses', label: apiLabels.responses },
  { value: 'completions', label: apiLabels.completions },
  { value: 'opencode-go', label: apiLabels['opencode-go'] },
  { value: 'decision', label: apiLabels.decision },
]

/** Provider | API | Base URL | Key | Models | actions, shared by the header and every row. */
const gridClassName = 'sm:grid-cols-[minmax(0,0.8fr)_152px_minmax(0,1fr)_96px_88px_88px]'

/** The Providers settings page. */
export function ProvidersPage() {
  const result = useAtomValue(providersPageAtom)
  const retry = useAtomRefresh(providersPageAtom)
  return matchCached(result, {
    onInitial: () => (
      <Page title="Providers" width="wide">
        <EmptyLine>Loading…</EmptyLine>
      </Page>
    ),
    onFailure: failure => (
      <Page title="Providers" width="wide">
        <PageError cause={failure.cause} onRetry={retry} />
      </Page>
    ),
    onSuccess: value => (
      <ProvidersOwner.Provider input={{ providers: value }}>
        <ProvidersContent />
      </ProvidersOwner.Provider>
    ),
  })
}

function ProvidersContent() {
  const machine = ProvidersOwner.useMachine()
  const send = useAtomSet(machine.send)
  useEffect(() => {
    send(ProvidersEvents.Refresh())
  }, [send])
  const addButtonRef = useRef<HTMLButtonElement>(null)
  const isAddClosed = AsyncResult.getOrElse(
    useMachineResult(AtomMachine.matches(machine, 'Add.Closed')),
    () => true,
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
      title="Providers"
      width="wide"
      actions={
        <MachineState machine={machine} path="Add.Closed">
          {() => (
            <Button ref={addButtonRef} onClick={() => send(ProvidersEvents.OpenAdd())}>
              <Plus data-icon="inline-start" />
              Add provider
            </Button>
          )}
        </MachineState>
      }
    >
      <MachineState machine={machine} path="Add.Closed" inactive={<AddSection />}>
        {() => null}
      </MachineState>
      <ProvidersList />
      <EditKeyDialogHost />
    </Page>
  )
}

function ProvidersList() {
  const machine = ProvidersOwner.useMachine()
  return (
    <MachineState machine={machine} path="">
      {({ value, states }) =>
        value.providers.length === 0 ? (
          <EmptyLine>No providers yet.</EmptyLine>
        ) : (
          <div>
            <ProvidersHeader />
            <ul className="flex flex-col divide-y">
              {value.providers.map(provider => (
                <ProviderRow
                  key={provider.id}
                  provider={provider}
                  refreshingId={value.refreshingId}
                  busy={
                    states.Action.state.path !== 'Action.Idle' ||
                    states.KeyDialog.state.path === 'KeyDialog.Submitting' ||
                    states.Add.state.path === 'Add.Submitting'
                  }
                />
              ))}
            </ul>
          </div>
        )
      }
    </MachineState>
  )
}

function ProvidersHeader() {
  return (
    <div
      className={cn(
        'hidden h-10 items-center gap-x-3 border-b border-border px-2 text-sm font-medium text-muted-foreground sm:grid',
        gridClassName,
      )}
    >
      <span>Provider</span>
      <span>API</span>
      <span>Base URL</span>
      <span>Key</span>
      <span className="text-right">Models</span>
      <span />
    </div>
  )
}

const modelCountLabel = (count: number): string => `${count} ${count === 1 ? 'model' : 'models'}`

interface ProviderRowProps {
  readonly provider: Provider
  readonly refreshingId: string | null
  readonly busy: boolean
}

function ProviderRow(props: ProviderRowProps) {
  const machine = ProvidersOwner.useMachine()
  const send = useAtomSet(machine.send)
  const { provider, refreshingId, busy } = props
  const statusText =
    refreshingId === provider.id ? 'Refreshing…' : modelCountLabel(provider.modelCount)
  return (
    <li
      className={cn(
        'group grid grid-cols-1 items-center gap-x-3 gap-y-2.5 px-1 py-3 sm:min-h-12 sm:gap-y-1.5 sm:px-2 sm:py-2',
        gridClassName,
      )}
    >
      <div className="flex min-w-0 items-center">
        <span className="min-w-0 break-words text-sm font-medium text-foreground [overflow-wrap:anywhere] sm:truncate">
          {provider.id}
        </span>
      </div>
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2 sm:contents">
        <div className="flex min-w-0 items-center">
          <span className="rounded-full bg-muted px-2 py-px text-xs font-medium whitespace-nowrap text-foreground">
            {apiLabels[provider.api]}
          </span>
        </div>
        <div className="flex w-full min-w-0 items-center gap-1.5 text-muted-foreground">
          <Link2 className="size-3.5 shrink-0" />
          <span className="min-w-0 break-words font-mono text-xs select-text [overflow-wrap:anywhere] sm:truncate">
            {provider.baseUrl}
          </span>
        </div>
        <div className="flex min-w-0 items-center gap-1.5 text-muted-foreground">
          <KeyRound className="size-3.5 shrink-0" />
          <span className="min-w-0 text-xs whitespace-nowrap">
            {provider.apiKey === 'set' ? 'API key set' : 'No API key'}
          </span>
        </div>
        <div className="flex shrink-0 items-center justify-end">
          <span aria-live="polite" className="text-xs text-muted-foreground tabular-nums">
            {statusText}
          </span>
        </div>
        <div className="flex shrink-0 items-center justify-end gap-1">
          <Tooltip>
            <TooltipTrigger
              render={
                <Button
                  variant="ghost"
                  size="icon-xs"
                  aria-label={`Update API key for ${provider.id}`}
                  className="text-muted-foreground"
                  disabled={busy}
                  onClick={() => send(ProvidersEvents.OpenKeyDialog({ id: provider.id }))}
                >
                  <Pencil />
                </Button>
              }
            />
            <TooltipContent>Update API key</TooltipContent>
          </Tooltip>
          <Tooltip>
            <TooltipTrigger
              render={
                <Button
                  variant="ghost"
                  size="icon-xs"
                  aria-label={`Refresh ${provider.id} models`}
                  className="text-muted-foreground"
                  disabled={busy}
                  onClick={() => send(ProvidersEvents.RefreshProvider({ id: provider.id }))}
                >
                  <RefreshCw />
                </Button>
              }
            />
            <TooltipContent>Refresh models</TooltipContent>
          </Tooltip>
          <ConfirmDialog
            tooltip="Remove provider"
            trigger={
              <Button
                variant="ghost"
                size="icon-xs"
                aria-label={`Remove ${provider.id}`}
                className="text-muted-foreground hover:bg-destructive/10 hover:text-destructive"
                disabled={busy}
              >
                <Trash2 />
              </Button>
            }
            title="Remove provider?"
            detail={provider.id}
            confirmLabel="Remove"
            onConfirm={() => send(ProvidersEvents.RemoveProvider({ id: provider.id }))}
          />
        </div>
      </div>
    </li>
  )
}

function EditKeyDialogHost() {
  const machine = ProvidersOwner.useMachine()
  return (
    <MachineState machine={machine} path="">
      {({ value, states }) => {
        const keyState = states.KeyDialog.state
        const activeId =
          keyState.path === 'KeyDialog.Editing' || keyState.path === 'KeyDialog.Submitting'
            ? keyState.value.id
            : null
        const provider =
          activeId === null ? null : (value.providers.find(p => p.id === activeId) ?? null)
        const error = keyState.path === 'KeyDialog.Editing' ? keyState.value.error : null
        const submitting = keyState.path === 'KeyDialog.Submitting'
        return (
          <EditKeyDialog
            key={activeId === null ? 'closed' : `open:${activeId}`}
            provider={provider}
            submitting={submitting}
            error={error}
          />
        )
      }}
    </MachineState>
  )
}

interface EditKeyDialogProps {
  readonly provider: Provider | null
  readonly submitting: boolean
  readonly error: string | null
}

function EditKeyDialog(props: EditKeyDialogProps) {
  const machine = ProvidersOwner.useMachine()
  const send = useAtomSet(machine.send)
  const [apiKey, setApiKey] = useState('')
  const { provider, submitting, error } = props

  const submit = (event: FormEvent) => {
    event.preventDefault()
    if (provider === null) return
    const trimmed = apiKey.trim()
    if (trimmed === '') return
    send(ProvidersEvents.SubmitKey({ id: provider.id, apiKey: trimmed }))
  }

  return (
    <Dialog
      open={provider !== null}
      onOpenChange={open => {
        if (!open && !submitting) send(ProvidersEvents.CloseKeyDialog())
      }}
    >
      <DialogContent className="sm:max-w-md">
        {provider === null ? null : (
          <form onSubmit={submit} className="space-y-4">
            <DialogHeader>
              <DialogTitle>API key · {provider.id}</DialogTitle>
            </DialogHeader>
            <div className="space-y-2">
              <Input
                id="provider-edit-key"
                type="password"
                autoComplete="off"
                className="font-mono"
                placeholder="Paste new API key"
                value={apiKey}
                disabled={submitting}
                aria-invalid={error !== null}
                aria-describedby={error === null ? undefined : 'provider-edit-key-error'}
                onChange={event => {
                  setApiKey(event.target.value)
                  if (error !== null) send(ProvidersEvents.ClearKeyError())
                }}
              />
              {error === null ? null : (
                <FieldError id="provider-edit-key-error">{error}</FieldError>
              )}
            </div>
            <DialogFooter>
              {provider.apiKey === 'set' ? (
                <Button
                  type="button"
                  variant="destructive"
                  disabled={submitting}
                  className="sm:mr-auto"
                  onClick={() => send(ProvidersEvents.SubmitKey({ id: provider.id, apiKey: null }))}
                >
                  Clear key
                </Button>
              ) : null}
              <Button
                type="button"
                variant="ghost"
                disabled={submitting}
                onClick={() => send(ProvidersEvents.CloseKeyDialog())}
              >
                Cancel
              </Button>
              <Button type="submit" disabled={submitting || apiKey.trim() === ''}>
                {submitting ? 'Saving…' : 'Save'}
              </Button>
            </DialogFooter>
          </form>
        )}
      </DialogContent>
    </Dialog>
  )
}

function AddSection() {
  const machine = ProvidersOwner.useMachine()
  return (
    <Section title="New provider" className="mb-8 max-w-[720px]">
      <MachineState machine={machine} path="">
        {({ value }) => <AddForm existingIds={value.providers.map(provider => provider.id)} />}
      </MachineState>
    </Section>
  )
}

interface AddDraft {
  readonly id: string
  readonly api: ProviderApi
  readonly baseUrl: string
  readonly apiKey: string
  readonly models: string
}

type RequiredField = 'id' | 'baseUrl'
type FieldErrors = Partial<Record<RequiredField, string>>

const fieldElementIds = {
  id: 'provider-id',
  baseUrl: 'provider-base-url',
} satisfies Record<RequiredField, string>

const validateProviderDraft = (draft: AddDraft, existingIds: ReadonlyArray<string>) => {
  const errors: FieldErrors = {}
  const id = draft.id.trim()
  if (id === '') {
    errors.id = 'Required'
  } else if (id.includes('/')) {
    errors.id = "Provider ID cannot contain '/'."
  } else if (existingIds.includes(id)) {
    errors.id = 'A provider with this ID already exists.'
  }
  const baseUrl = draft.baseUrl.trim()
  if (baseUrl === '') {
    if (draft.api !== 'opencode-go') errors.baseUrl = 'Required'
  } else if (!URL.canParse(baseUrl)) {
    errors.baseUrl = 'Enter a valid URL.'
  }
  const order: ReadonlyArray<RequiredField> = ['id', 'baseUrl']
  const firstField = order.find(field => errors[field] !== undefined) ?? null
  return { errors, firstField }
}

interface AddFormProps {
  readonly existingIds: ReadonlyArray<string>
}

function AddForm(props: AddFormProps) {
  const machine = ProvidersOwner.useMachine()
  const send = useAtomSet(machine.send)
  const idRef = useRef<HTMLInputElement>(null)
  useEffect(() => {
    idRef.current?.focus()
  }, [])
  const [draft, setDraft] = useState<AddDraft>({
    id: '',
    api: 'responses',
    baseUrl: '',
    apiKey: '',
    models: '',
  })
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

  const editField = (field: RequiredField, value: string) => {
    setDraft(current => ({ ...current, [field]: value }))
    setErrors(current => {
      if (current[field] === undefined) return current
      const next = { ...current }
      delete next[field]
      return next
    })
    if (serverError != null) send(ProvidersEvents.ClearAddError())
  }

  const submit = (event: FormEvent) => {
    event.preventDefault()
    const validation = validateProviderDraft(draft, props.existingIds)
    setErrors(validation.errors)
    if (validation.firstField !== null) {
      document.getElementById(fieldElementIds[validation.firstField])?.focus()
      return
    }
    const trimmedKey = draft.apiKey.trim()
    const models =
      draft.api === 'decision'
        ? draft.models
            .split(',')
            .map(item => item.trim())
            .filter(item => item !== '')
        : []
    send(
      ProvidersEvents.SubmitAdd({
        id: draft.id.trim(),
        api: draft.api,
        baseUrl: draft.baseUrl.trim(),
        apiKey: trimmedKey === '' ? null : trimmedKey,
        models,
      }),
    )
  }

  return (
    <form noValidate onSubmit={submit}>
      <FieldRow id="provider-id" label="ID">
        <Input
          ref={idRef}
          id="provider-id"
          className="font-mono"
          value={draft.id}
          disabled={isSubmitting}
          aria-invalid={errors.id !== undefined}
          aria-describedby={errors.id === undefined ? undefined : 'provider-id-error'}
          onChange={event => editField('id', event.target.value)}
        />
        {errors.id === undefined ? null : (
          <FieldError id="provider-id-error">{errors.id}</FieldError>
        )}
      </FieldRow>
      <FieldRow id="provider-api" label="API">
        <Select
          value={draft.api}
          items={apiOptions}
          disabled={isSubmitting}
          onValueChange={api => {
            if (api !== null) {
              setDraft(current => ({ ...current, api }))
              if (serverError != null) send(ProvidersEvents.ClearAddError())
            }
          }}
        >
          <SelectTrigger id="provider-api" className="w-full">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectGroup>
              {apiOptions.map(option => (
                <SelectItem key={option.value} value={option.value}>
                  {option.label}
                </SelectItem>
              ))}
            </SelectGroup>
          </SelectContent>
        </Select>
      </FieldRow>
      <FieldRow id="provider-base-url" label="Base URL">
        <Input
          id="provider-base-url"
          className="font-mono"
          placeholder={
            draft.api === 'decision'
              ? 'https://api.cloudflare.com/client/v4/accounts/<id>/ai/run'
              : draft.api === 'opencode-go'
                ? 'https://opencode.ai/inference/go'
                : 'https://…/v1'
          }
          value={draft.baseUrl}
          disabled={isSubmitting}
          aria-invalid={errors.baseUrl !== undefined}
          aria-describedby={errors.baseUrl === undefined ? undefined : 'provider-base-url-error'}
          onChange={event => editField('baseUrl', event.target.value)}
        />
        {errors.baseUrl === undefined ? null : (
          <FieldError id="provider-base-url-error">{errors.baseUrl}</FieldError>
        )}
      </FieldRow>
      <FieldRow id="provider-api-key" label="API key">
        <Input
          id="provider-api-key"
          type="password"
          autoComplete="off"
          className="font-mono"
          placeholder="Optional for local endpoints"
          value={draft.apiKey}
          disabled={isSubmitting}
          onChange={event => {
            setDraft(current => ({ ...current, apiKey: event.target.value }))
            if (serverError != null) send(ProvidersEvents.ClearAddError())
          }}
        />
      </FieldRow>
      {draft.api === 'decision' ? (
        <FieldRow id="provider-models" label="Models">
          <Input
            id="provider-models"
            className="font-mono"
            placeholder="Auto-seeded for Cloudflare /ai/run, or comma-separated IDs"
            value={draft.models}
            disabled={isSubmitting}
            onChange={event => {
              setDraft(current => ({ ...current, models: event.target.value }))
              if (serverError != null) send(ProvidersEvents.ClearAddError())
            }}
          />
        </FieldRow>
      ) : null}
      <div className="mt-3 flex flex-col items-start gap-2 md:pl-[120px]">
        <AddButtons />
        <AddError />
      </div>
    </form>
  )
}

function AddButtons() {
  const machine = ProvidersOwner.useMachine()
  const send = useAtomSet(machine.send)
  return (
    <div className="flex items-center gap-2">
      <MachineState
        machine={machine}
        path="Add.Submitting"
        inactive={
          <>
            <Button type="submit">Add provider</Button>
            <Button
              type="button"
              variant="secondary"
              onClick={() => send(ProvidersEvents.CloseAdd())}
            >
              Cancel
            </Button>
          </>
        }
      >
        {() => (
          <>
            <Button type="submit" disabled>
              Adding provider…
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
  const machine = ProvidersOwner.useMachine()
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
