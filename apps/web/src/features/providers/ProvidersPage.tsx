import type { Provider, ProviderApi } from '@blob-1/contracts'
import { useAtomRefresh, useAtomSet, useAtomValue } from '@effect/atom-react'
import { AtomMachine } from '@typeonce/effect-machine/reactivity'
import {
  createMachineContext,
  MachineState,
  useMachineResult,
} from '@typeonce/effect-machine-react'
import { Option } from 'effect'
import { AsyncResult } from 'effect/reactivity'
import { KeyRound, Link2, Plus, RefreshCw, Server, Trash2 } from 'lucide-react'
import { type FormEvent, type ReactNode, useEffect, useRef, useState } from 'react'

import { ConfirmDialog } from '~/components/ConfirmDialog'
import { EmptyLine, FieldError, FieldRow, Page, PageError, Section } from '~/components/layout/Page'
import { Button } from '~/components/ui/button'
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

import { ProvidersEvents, type ProvidersInput, ProvidersMachine } from './providersMachine'
import { providersPageAtom } from './providersState'

const ProvidersOwner = createMachineContext(({ input }: { input: ProvidersInput }) =>
  MachineAtoms.make(ProvidersMachine, input),
)

const apiLabels = {
  responses: 'Responses',
  completions: 'Chat Completions',
}

const apiOptions = [
  { value: 'responses', label: apiLabels.responses },
  { value: 'completions', label: apiLabels.completions },
]

/** The Providers settings page. */
export function ProvidersPage() {
  const result = useAtomValue(providersPageAtom)
  const retry = useAtomRefresh(providersPageAtom)
  return AsyncResult.match(result, {
    onInitial: () => (
      <Page title="Providers" wide>
        <EmptyLine>Loading…</EmptyLine>
      </Page>
    ),
    onFailure: failure => (
      <Page title="Providers" wide>
        <PageError cause={failure.cause} onRetry={retry} />
      </Page>
    ),
    onSuccess: ({ value }) => (
      <ProvidersOwner.Provider input={{ providers: value }}>
        <ProvidersContent />
      </ProvidersOwner.Provider>
    ),
  })
}

function ProvidersContent() {
  const machine = ProvidersOwner.useMachine()
  const send = useAtomSet(machine.send)
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
      wide
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
          <ul className="divide-y divide-border overflow-hidden rounded-2xl border border-border bg-card shadow-xs">
            {value.providers.map(provider => (
              <ProviderRow
                key={provider.id}
                provider={provider}
                refreshingId={value.refreshingId}
                removing={states.Remove.state.path === 'Remove.Working'}
              />
            ))}
          </ul>
        )
      }
    </MachineState>
  )
}

const modelCountLabel = (count: number): string => `${count} ${count === 1 ? 'model' : 'models'}`

interface DetailProps {
  readonly icon: ReactNode
  readonly text: string
}

function Detail(props: DetailProps) {
  return (
    <span className="flex min-w-0 items-start gap-1.5">
      <span className="mt-0.5 shrink-0">{props.icon}</span>
      <span className="min-w-0 break-words font-mono text-xs select-text [overflow-wrap:anywhere]">
        {props.text}
      </span>
    </span>
  )
}

interface ProviderRowProps {
  readonly provider: Provider
  readonly refreshingId: string | null
  readonly removing: boolean
}

function ProviderRow(props: ProviderRowProps) {
  const machine = ProvidersOwner.useMachine()
  const send = useAtomSet(machine.send)
  const { provider, refreshingId, removing } = props
  const statusText =
    refreshingId === provider.id ? 'Refreshing…' : modelCountLabel(provider.modelCount)
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
            {provider.id}
          </p>
          <span className="rounded-full bg-muted px-2 py-px text-xs font-medium text-foreground">
            {apiLabels[provider.api]}
          </span>
        </div>
        <div className="mt-1.5 flex min-w-0 flex-col gap-1 text-muted-foreground sm:mt-1 sm:flex-row sm:items-center sm:gap-4">
          <Detail icon={<Link2 className="size-3.5" />} text={provider.baseUrl} />
          <Detail icon={<KeyRound className="size-3.5" />} text={provider.keyEnv} />
        </div>
      </div>
      <div className="flex shrink-0 items-center justify-between gap-3 sm:justify-end sm:gap-4">
        <span aria-live="polite" className="shrink-0 text-xs text-muted-foreground tabular-nums">
          {statusText}
        </span>
        <div className="flex shrink-0 items-center gap-3 sm:gap-2">
          <Tooltip>
            <TooltipTrigger
              render={
                <Button
                  variant="outline"
                  size="icon"
                  aria-label={`Refresh ${provider.id} models`}
                  disabled={refreshingId !== null}
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
                variant="outline"
                size="icon"
                aria-label={`Remove ${provider.id}`}
                className="text-destructive"
                disabled={removing}
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
  readonly keyEnv: string
}

type RequiredField = 'id' | 'baseUrl' | 'keyEnv'
type FieldErrors = Partial<Record<RequiredField, string>>

const fieldElementIds = {
  id: 'provider-id',
  baseUrl: 'provider-base-url',
  keyEnv: 'provider-key-env',
} satisfies Record<RequiredField, string>

const validateProviderDraft = (draft: AddDraft, existingIds: ReadonlyArray<string>) => {
  const errors: FieldErrors = {}
  const id = draft.id.trim()
  if (id === '') {
    errors.id = 'Required'
  } else if (existingIds.includes(id)) {
    errors.id = 'A provider with this ID already exists.'
  }
  const baseUrl = draft.baseUrl.trim()
  if (baseUrl === '') {
    errors.baseUrl = 'Required'
  } else if (!URL.canParse(baseUrl)) {
    errors.baseUrl = 'Enter a valid URL.'
  }
  if (draft.keyEnv.trim() === '') {
    errors.keyEnv = 'Required'
  }
  const order: ReadonlyArray<RequiredField> = ['id', 'baseUrl', 'keyEnv']
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
    keyEnv: '',
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
    send(
      ProvidersEvents.SubmitAdd({
        id: draft.id.trim(),
        api: draft.api,
        baseUrl: draft.baseUrl.trim(),
        keyEnv: draft.keyEnv.trim(),
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
          placeholder="https://…/v1"
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
      <FieldRow id="provider-key-env" label="Key env var">
        <Input
          id="provider-key-env"
          className="font-mono"
          placeholder="OPENAI_API_KEY"
          value={draft.keyEnv}
          disabled={isSubmitting}
          aria-invalid={errors.keyEnv !== undefined}
          aria-describedby={errors.keyEnv === undefined ? undefined : 'provider-key-env-error'}
          onChange={event => editField('keyEnv', event.target.value)}
        />
        {errors.keyEnv === undefined ? null : (
          <FieldError id="provider-key-env-error">{errors.keyEnv}</FieldError>
        )}
      </FieldRow>
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
