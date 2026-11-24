import type { Secret, SecretRequestLink } from '@blob-1/contracts'
import { useAtomRefresh, useAtomSet, useAtomValue } from '@effect/atom-react'
import { AtomMachine } from '@typeonce/effect-machine/reactivity'
import { MachineState, useMachineResult } from '@typeonce/effect-machine-react'
import { AsyncResult } from 'effect/reactivity'
import { Copy, Link2, RefreshCw, Trash2 } from 'lucide-react'
import { type FormEvent, useCallback, useEffect, useState } from 'react'
import { toast } from 'sonner'

import { ConfirmDialog } from '~/components/ConfirmDialog'
import { EmptyLine, FieldError, PageError } from '~/components/layout/Page'
import { SplitDetail } from '~/components/layout/SplitPage'
import { Button } from '~/components/ui/button'
import { Input } from '~/components/ui/input'
import { Tooltip, TooltipContent, TooltipTrigger } from '~/components/ui/tooltip'
import { useBotInstanceKey, useBotsRoot } from '~/features/bots/CreateBotDialog'
import { machineAtomsForBot } from '~/lib/runtime'
import { createPersistentMachineContext } from '~/lib/shellMachines'
import { matchCached, useChangeRefresh } from '~/lib/useBackgroundRefresh'

import { SecretsEvents, type SecretsInput, SecretsMachine } from './secretsMachine'
import { secretsPageAtomFor } from './secretsState'

/** Mirrors the server's rule: names become file names, so no leading dot. */
const SECRET_NAME = /^[A-Za-z0-9_-][A-Za-z0-9_.-]{0,63}$/
const SECRET_NAME_ERROR =
  'Use up to 64 letters, numbers, dots, underscores or hyphens, not starting with a dot.'

const SecretsOwner = createPersistentMachineContext(
  'secrets',
  ({ input, scopeKey }: { readonly input: SecretsInput; readonly scopeKey: string }) =>
    machineAtomsForBot(scopeKey).make(SecretsMachine, input),
)

interface SecretsPageProps {
  readonly botId: string
}

/** The Secrets settings page for a bot. Values are write-only: none ever reach the browser. */
export function SecretsPage(props: SecretsPageProps) {
  const instanceKey = useBotInstanceKey(props.botId)
  const atom = secretsPageAtomFor(instanceKey)
  const result = useAtomValue(atom)
  const retry = useAtomRefresh(atom)
  const bots = useBotsRoot()?.bots ?? []
  const botName = bots.find(entry => entry.id === props.botId)?.name ?? props.botId

  return matchCached(result, {
    onInitial: () => (
      <SplitDetail
        title="Secrets"
        wide
        isListView={false}
        backTo="/settings/bots/$botId"
        backParams={{ botId: props.botId }}
        backLabel={botName}
      >
        <EmptyLine>Loading…</EmptyLine>
      </SplitDetail>
    ),
    onFailure: failure => (
      <SplitDetail
        title="Secrets"
        wide
        isListView={false}
        backTo="/settings/bots/$botId"
        backParams={{ botId: props.botId }}
        backLabel={botName}
      >
        <PageError cause={failure.cause} onRetry={retry} />
      </SplitDetail>
    ),
    onSuccess: payload => (
      <SecretsOwner.Provider input={payload} scopeKey={instanceKey}>
        <SecretsContent botId={props.botId} botName={botName} instanceKey={instanceKey} />
      </SecretsOwner.Provider>
    ),
  })
}

interface SecretsContentProps {
  readonly botId: string
  readonly botName: string
  readonly instanceKey: string
}

function SecretsContent(props: SecretsContentProps) {
  const machine = SecretsOwner.useMachine()
  const send = useAtomSet(machine.send)
  useEffect(() => {
    send(SecretsEvents.Refresh())
  }, [send])
  const refresh = useCallback(() => send(SecretsEvents.Refresh()), [send])
  useChangeRefresh(props.instanceKey, 'secrets', refresh)

  const isRefreshing = AsyncResult.getOrElse(
    useMachineResult(AtomMachine.matches(machine, 'Refresh.Working')),
    () => false,
  )

  return (
    <SplitDetail
      title="Secrets"
      wide
      isListView={false}
      backTo="/settings/bots/$botId"
      backParams={{ botId: props.botId }}
      backLabel={props.botName}
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
                  aria-label="Refresh secrets"
                  disabled={isRefreshing}
                  onClick={refresh}
                >
                  <RefreshCw />
                </Button>
              }
            />
            <TooltipContent>{isRefreshing ? 'Refreshing…' : 'Refresh secrets'}</TooltipContent>
          </Tooltip>
        </>
      }
    >
      <MachineState machine={machine} path="">
        {({ value, states }) => {
          const writing = value.inFlightWrites > 0
          const requesting = states.Request.state.path === 'Request.Working'
          const canRequest = value.publicUrl !== null && !requesting
          return (
            <div className="space-y-6">
              {value.publicUrl === null ? (
                <p className="rounded-2xl border border-border bg-muted/50 px-4 py-3 text-xs text-muted-foreground">
                  Start with <span className="font-mono text-foreground">--public-url</span> to
                  create request links.
                </p>
              ) : null}
              <NewSecretRow
                key={value.saveSeq}
                writing={writing}
                canRequest={canRequest}
                saveError={value.mutationErrorKind === 'set' ? value.mutationError : null}
                requestError={value.requestError}
              />
              {value.requestLink === null ? null : <RequestLink link={value.requestLink} />}
              {value.mutationErrorKind === 'remove' && value.mutationError !== null ? (
                <FieldError>{value.mutationError}</FieldError>
              ) : null}
              <SecretsList secrets={value.secrets} writing={writing} canRequest={canRequest} />
            </div>
          )
        }}
      </MachineState>
    </SplitDetail>
  )
}

interface NewSecretRowProps {
  readonly writing: boolean
  readonly canRequest: boolean
  readonly saveError: string | null
  readonly requestError: string | null
}

/** Saves a value under a name, or asks someone else for it with a one-time link. */
function NewSecretRow(props: NewSecretRowProps) {
  const machine = SecretsOwner.useMachine()
  const send = useAtomSet(machine.send)
  const [name, setName] = useState('')
  const [value, setValue] = useState('')
  const [localError, setLocalError] = useState<string | null>(null)
  const error = localError ?? props.saveError ?? props.requestError

  const clearErrors = () => {
    if (localError !== null) setLocalError(null)
    if (props.saveError !== null) send(SecretsEvents.ClearMutationError())
    if (props.requestError !== null) send(SecretsEvents.ClearRequestError())
  }

  const validName = (): string | null => {
    const trimmed = name.trim()
    if (!SECRET_NAME.test(trimmed)) {
      setLocalError(SECRET_NAME_ERROR)
      return null
    }
    return trimmed
  }

  const handleSave = (event: FormEvent) => {
    event.preventDefault()
    const valid = validName()
    if (valid === null) return
    if (value.trim() === '') {
      setLocalError('Enter a value.')
      return
    }
    send(SecretsEvents.SetSecret({ name: valid, value }))
  }

  const handleRequest = () => {
    const valid = validName()
    if (valid !== null) send(SecretsEvents.RequestSecret({ name: valid, label: null }))
  }

  return (
    <form noValidate onSubmit={handleSave} className="max-w-[640px]">
      <div className="flex flex-wrap items-center gap-2 sm:flex-nowrap">
        <Input
          aria-label="Secret name"
          placeholder="Name"
          value={name}
          disabled={props.writing}
          aria-invalid={error !== null}
          aria-describedby={error === null ? undefined : 'secret-error'}
          onChange={event => {
            setName(event.target.value)
            clearErrors()
          }}
        />
        <Input
          aria-label="Secret value"
          type="password"
          autoComplete="off"
          placeholder="Value"
          value={value}
          disabled={props.writing}
          aria-invalid={error !== null}
          aria-describedby={error === null ? undefined : 'secret-error'}
          onChange={event => {
            setValue(event.target.value)
            clearErrors()
          }}
        />
        <Button type="submit" disabled={props.writing}>
          {props.writing ? 'Saving…' : 'Save'}
        </Button>
        <Button
          type="button"
          variant="outline"
          disabled={!props.canRequest}
          onClick={handleRequest}
        >
          <Link2 data-icon="inline-start" />
          Request link
        </Button>
      </div>
      {error === null ? null : <FieldError id="secret-error">{error}</FieldError>}
    </form>
  )
}

interface RequestLinkProps {
  readonly link: SecretRequestLink
}

/** The last request link, to hand to whoever holds the secret. */
function RequestLink(props: RequestLinkProps) {
  const expires = new Date(props.link.expiresAt * 1000)
  const handleCopy = () => {
    navigator.clipboard.writeText(props.link.url).then(
      () => toast.success('Link copied'),
      () => toast.error('Could not copy link'),
    )
  }

  return (
    <div className="max-w-[640px] rounded-2xl border border-border bg-card px-4 py-3 shadow-xs">
      <div className="flex min-w-0 items-center gap-1.5">
        <span className="min-w-0 flex-1 truncate font-mono text-xs text-foreground select-all">
          {props.link.url}
        </span>
        <Tooltip>
          <TooltipTrigger
            render={
              <Button
                type="button"
                variant="ghost"
                size="icon-xs"
                aria-label="Copy request link"
                onClick={handleCopy}
              >
                <Copy />
              </Button>
            }
          />
          <TooltipContent>Copy link</TooltipContent>
        </Tooltip>
      </div>
      <time
        className="mt-1 block text-xs text-muted-foreground tabular-nums"
        dateTime={expires.toISOString()}
      >
        Expires {expires.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}
      </time>
    </div>
  )
}

interface SecretsListProps {
  readonly secrets: ReadonlyArray<Secret>
  readonly writing: boolean
  readonly canRequest: boolean
}

function SecretsList(props: SecretsListProps) {
  if (props.secrets.length === 0) {
    return <EmptyLine>No secrets configured yet.</EmptyLine>
  }

  return (
    <ul className="divide-y divide-border overflow-hidden rounded-2xl border border-border bg-card shadow-xs">
      {props.secrets.map(secret => (
        <SecretRow
          key={secret.name}
          secret={secret}
          writing={props.writing}
          canRequest={props.canRequest}
        />
      ))}
    </ul>
  )
}

interface SecretRowProps {
  readonly secret: Secret
  readonly writing: boolean
  readonly canRequest: boolean
}

function SecretRow(props: SecretRowProps) {
  const machine = SecretsOwner.useMachine()
  const send = useAtomSet(machine.send)
  const updated = new Date(props.secret.updatedAt * 1000)

  return (
    <li className="flex items-center gap-2 px-4 py-3">
      <div className="min-w-0 flex-1">
        <p className="truncate font-mono text-sm text-foreground">{props.secret.name}</p>
        <time
          className="block text-xs text-muted-foreground tabular-nums"
          dateTime={updated.toISOString()}
        >
          Updated {updated.toLocaleString()}
        </time>
      </div>
      <Tooltip>
        <TooltipTrigger
          render={
            <Button
              variant="ghost"
              size="icon"
              aria-label={`Request a new value for ${props.secret.name}`}
              disabled={!props.canRequest}
              onClick={() =>
                send(SecretsEvents.RequestSecret({ name: props.secret.name, label: null }))
              }
            >
              <Link2 />
            </Button>
          }
        />
        <TooltipContent>Request a new value</TooltipContent>
      </Tooltip>
      <ConfirmDialog
        tooltip="Remove secret"
        trigger={
          <Button
            variant="ghost"
            size="icon"
            aria-label={`Remove ${props.secret.name}`}
            className="text-destructive hover:bg-destructive/10 hover:text-destructive"
            disabled={props.writing}
          >
            <Trash2 />
          </Button>
        }
        title="Remove secret?"
        detail={props.secret.name}
        confirmLabel="Remove"
        onConfirm={() => send(SecretsEvents.RemoveSecret({ name: props.secret.name }))}
      />
    </li>
  )
}
