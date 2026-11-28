import type { Webhook } from '@blob-1/contracts'
import { useAtomRefresh, useAtomSet, useAtomValue } from '@effect/atom-react'
import { AtomMachine } from '@typeonce/effect-machine/reactivity'
import { MachineState, useMachineResult } from '@typeonce/effect-machine-react'
import { Option } from 'effect'
import { AsyncResult } from 'effect/reactivity'
import { Copy, Plus, RefreshCw, Trash2, Webhook as WebhookIcon } from 'lucide-react'
import { type FormEvent, useCallback, useEffect, useState } from 'react'
import { toast } from 'sonner'

import { ConfirmDialog } from '~/components/ConfirmDialog'
import { EmptyLine, FieldError, PageError } from '~/components/layout/Page'
import { SplitDetail } from '~/components/layout/SplitPage'
import { Button } from '~/components/ui/button'
import { Input } from '~/components/ui/input'
import { Switch } from '~/components/ui/switch'
import { Tooltip, TooltipContent, TooltipTrigger } from '~/components/ui/tooltip'
import { useBotInstanceKey, useBotsRoot } from '~/features/bots/CreateBotDialog'
import { machineAtomsForBot } from '~/lib/runtime'
import { createPersistentMachineContext } from '~/lib/shellMachines'
import { matchCached, useChangeRefresh } from '~/lib/useBackgroundRefresh'

import { WebhooksEvents, type WebhooksInput, WebhooksMachine } from './webhooksMachine'
import { webhooksPageAtomFor } from './webhooksState'

const WebhooksOwner = createPersistentMachineContext(
  'webhooks',
  ({ input, scopeKey }: { readonly input: WebhooksInput; readonly scopeKey: string }) =>
    machineAtomsForBot(scopeKey).make(WebhooksMachine, input),
)

function buildEndpointUrl(publicUrl: string | null, token: string): string {
  const base = publicUrl === null ? '' : publicUrl.replace(/\/+$/, '')
  return `${base}/hooks/${token}`
}

function formatLastTriggered(timestampSecs: number | null): string {
  if (timestampSecs === null) return 'Never'
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

interface WebhooksPageProps {
  readonly botId: string
}

/** The Webhooks settings page for a bot. */
export function WebhooksPage(props: WebhooksPageProps) {
  const instanceKey = useBotInstanceKey(props.botId)
  const atom = webhooksPageAtomFor(instanceKey)
  const result = useAtomValue(atom)
  const retry = useAtomRefresh(atom)
  const bots = useBotsRoot()?.bots ?? []
  const botName = bots.find(entry => entry.id === props.botId)?.name ?? props.botId

  return matchCached(result, {
    onInitial: () => (
      <SplitDetail
        title="Webhooks"
        width="wide"
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
        title="Webhooks"
        width="wide"
        isListView={false}
        backTo="/settings/bots/$botId"
        backParams={{ botId: props.botId }}
        backLabel={botName}
      >
        <PageError cause={failure.cause} onRetry={retry} />
      </SplitDetail>
    ),
    onSuccess: payload => (
      <WebhooksOwner.Provider input={payload} scopeKey={instanceKey}>
        <WebhooksContent botId={props.botId} botName={botName} instanceKey={instanceKey} />
      </WebhooksOwner.Provider>
    ),
  })
}

interface WebhooksContentProps {
  readonly botId: string
  readonly botName: string
  readonly instanceKey: string
}

function WebhooksContent(props: WebhooksContentProps) {
  const machine = WebhooksOwner.useMachine()
  const send = useAtomSet(machine.send)
  useEffect(() => {
    send(WebhooksEvents.Refresh())
  }, [send])
  const refresh = useCallback(() => send(WebhooksEvents.Refresh()), [send])
  useChangeRefresh(props.instanceKey, 'webhooks', refresh)

  const isRefreshing = AsyncResult.getOrElse(
    useMachineResult(AtomMachine.matches(machine, 'Refresh.Working')),
    () => false,
  )

  return (
    <SplitDetail
      title="Webhooks"
      width="wide"
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
                  aria-label="Refresh webhooks"
                  disabled={isRefreshing}
                  onClick={() => send(WebhooksEvents.Refresh())}
                >
                  <RefreshCw />
                </Button>
              }
            />
            <TooltipContent>{isRefreshing ? 'Refreshing…' : 'Refresh webhooks'}</TooltipContent>
          </Tooltip>
        </>
      }
    >
      <MachineState machine={machine} path="">
        {({ value, states }) => (
          <div className="space-y-6">
            {value.publicUrl === null ? (
              <p className="rounded-2xl border border-border bg-muted/50 px-4 py-3 text-xs text-muted-foreground">
                Start with <span className="font-mono text-foreground">--public-url</span> to get
                public webhook URLs.
              </p>
            ) : null}
            <NewWebhookRow key={value.createSeq} />
            <WebhooksList
              publicUrl={value.publicUrl}
              webhooks={value.webhooks}
              removing={states.Remove.state.path === 'Remove.Working'}
            />
          </div>
        )}
      </MachineState>
    </SplitDetail>
  )
}

function NewWebhookRow() {
  const machine = WebhooksOwner.useMachine()
  const send = useAtomSet(machine.send)
  const [name, setName] = useState('')
  const [localError, setLocalError] = useState<string | null>(null)

  const isCreating = AsyncResult.getOrElse(
    useMachineResult(AtomMachine.matches(machine, 'Create.Working')),
    () => false,
  )
  const serverError = Option.getOrNull(
    AsyncResult.getOrElse(useMachineResult(AtomMachine.select(machine, 'Create.Idle')), () =>
      Option.none(),
    ),
  )?.error
  const error = localError ?? serverError ?? null

  const handleSubmit = (event: FormEvent) => {
    event.preventDefault()
    const trimmed = name.trim()
    if (trimmed === '') {
      setLocalError('Name is required.')
      return
    }
    setLocalError(null)
    send(WebhooksEvents.Create({ name: trimmed }))
  }

  return (
    <form noValidate onSubmit={handleSubmit} className="max-w-[520px]">
      <div className="flex items-center gap-2">
        <Input
          aria-label="New webhook name"
          placeholder="New webhook name"
          value={name}
          disabled={isCreating}
          aria-invalid={error !== null}
          aria-describedby={error === null ? undefined : 'webhook-create-error'}
          onChange={event => {
            setName(event.target.value)
            if (localError !== null) setLocalError(null)
            if (serverError != null) send(WebhooksEvents.ClearCreateError())
          }}
        />
        <Button type="submit" disabled={isCreating}>
          <Plus data-icon="inline-start" />
          {isCreating ? 'Adding…' : 'Add'}
        </Button>
      </div>
      {error === null ? null : <FieldError id="webhook-create-error">{error}</FieldError>}
    </form>
  )
}

interface WebhooksListProps {
  readonly publicUrl: string | null
  readonly webhooks: ReadonlyArray<Webhook>
  readonly removing: boolean
}

function WebhooksList(props: WebhooksListProps) {
  if (props.webhooks.length === 0) {
    return <EmptyLine>No webhooks configured yet.</EmptyLine>
  }

  return (
    <ul className="divide-y divide-border overflow-hidden rounded-2xl border border-border bg-card shadow-xs">
      {props.webhooks.map(webhook => (
        <WebhookRow
          key={webhook.id}
          webhook={webhook}
          publicUrl={props.publicUrl}
          removing={props.removing}
        />
      ))}
    </ul>
  )
}

interface WebhookRowProps {
  readonly webhook: Webhook
  readonly publicUrl: string | null
  readonly removing: boolean
}

function WebhookRow(props: WebhookRowProps) {
  const machine = WebhooksOwner.useMachine()
  const send = useAtomSet(machine.send)
  const { webhook, publicUrl, removing } = props
  const endpointUrl = buildEndpointUrl(publicUrl, webhook.token)

  const handleCopy = () => {
    navigator.clipboard.writeText(endpointUrl).then(
      () => toast.success('Webhook URL copied'),
      () => toast.error('Could not copy URL'),
    )
  }

  return (
    <li className="flex flex-col gap-3 p-4 sm:flex-row sm:items-center sm:gap-4">
      <span
        aria-hidden="true"
        className="hidden size-10 shrink-0 place-items-center rounded-full border border-border bg-shelf text-muted-foreground sm:grid"
      >
        <WebhookIcon className="size-5" />
      </span>
      <div className="min-w-0 flex-1 space-y-1">
        <div className="flex flex-wrap items-center gap-2">
          <p className="min-w-0 break-words text-sm font-medium text-foreground [overflow-wrap:anywhere]">
            {webhook.name}
          </p>
          <span className="inline-flex items-center gap-1.5 rounded-full bg-muted px-2 py-px text-xs font-medium text-foreground">
            <span
              aria-hidden="true"
              className={
                webhook.enabled
                  ? 'size-1.5 rounded-full bg-ok'
                  : 'size-1.5 rounded-full bg-muted-foreground/40'
              }
            />
            {webhook.enabled ? 'Enabled' : 'Disabled'}
          </span>
        </div>
        <div className="flex min-w-0 items-center gap-1.5">
          <span className="min-w-0 break-all font-mono text-xs text-muted-foreground select-all">
            {endpointUrl}
          </span>
          <Tooltip>
            <TooltipTrigger
              render={
                <Button
                  type="button"
                  variant="ghost"
                  size="icon-xs"
                  aria-label={`Copy URL for ${webhook.name}`}
                  onClick={handleCopy}
                >
                  <Copy />
                </Button>
              }
            />
            <TooltipContent>Copy URL</TooltipContent>
          </Tooltip>
        </div>
        <p className="text-xs text-muted-foreground tabular-nums">
          Last triggered: {formatLastTriggered(webhook.lastTriggeredAt)}
        </p>
      </div>
      <div className="flex shrink-0 items-center justify-end gap-3">
        <Switch
          aria-label={`Enable ${webhook.name}`}
          checked={webhook.enabled}
          onCheckedChange={enabled => send(WebhooksEvents.SetEnabled({ id: webhook.id, enabled }))}
        />
        <ConfirmDialog
          tooltip="Remove webhook"
          trigger={
            <Button
              variant="ghost"
              size="icon"
              aria-label={`Remove ${webhook.name}`}
              className="text-destructive hover:bg-destructive/10 hover:text-destructive"
              disabled={removing}
            >
              <Trash2 />
            </Button>
          }
          title="Remove webhook?"
          detail={webhook.name}
          confirmLabel="Remove"
          onConfirm={() => send(WebhooksEvents.Remove({ id: webhook.id }))}
        />
      </div>
    </li>
  )
}
