import type { BotSummary } from '@blob-1/contracts'
import { useAtomRefresh, useAtomSet, useAtomValue } from '@effect/atom-react'
import { Link, useBlocker } from '@tanstack/react-router'
import { AtomMachine } from '@typeonce/effect-machine/reactivity'
import { MachineState, useMachineResult } from '@typeonce/effect-machine-react'
import { Option } from 'effect'
import { AsyncResult } from 'effect/reactivity'
import { Trash2, Undo2 } from 'lucide-react'
import { type FormEvent, useEffect, useState } from 'react'

import { ConfirmDialog } from '~/components/ConfirmDialog'
import { EmptyLine, FieldRow, PageError, Section } from '~/components/layout/Page'
import { SplitDetail } from '~/components/layout/SplitPage'
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
import { Input } from '~/components/ui/input'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '~/components/ui/select'
import { Switch } from '~/components/ui/switch'
import { BotsEvents } from '~/features/bots/botsMachine'
import { BotsOwner, useBotInstanceKey, useBotsRoot } from '~/features/bots/CreateBotDialog'
import { environmentsPageAtom } from '~/features/environments/environmentsState'
import { machineAtomsForBot } from '~/lib/runtime'
import { createPersistentMachineContext } from '~/lib/shellMachines'
import { matchCached } from '~/lib/useBackgroundRefresh'
import { cn } from '~/lib/utils'

import { GeneralEvents, type GeneralInput, GeneralMachine } from './generalMachine'
import { generalPageAtomFor } from './generalState'
import { DiscordHomeFields, DiscordSettingsOwner } from '~/features/chat/DiscordSettings'
import {
  InstructionsEvents,
  type InstructionsInput,
  InstructionsMachine,
} from './instructionsMachine'
import { UsageSection } from './UsageSection'

const GeneralOwner = createPersistentMachineContext(
  'general',
  ({ input, scopeKey }: { readonly input: GeneralInput; readonly scopeKey: string }) =>
    machineAtomsForBot(scopeKey).make(GeneralMachine, input),
)

const InstructionsOwner = createPersistentMachineContext(
  'instructions',
  ({ input, scopeKey }: { readonly input: InstructionsInput; readonly scopeKey: string }) =>
    machineAtomsForBot(scopeKey).make(InstructionsMachine, input),
)

const resetAllowed = AtomMachine.can(InstructionsEvents.Reset())

const textClassName =
  'col-start-1 row-start-1 min-h-56 w-full px-3.5 py-3 whitespace-pre-wrap break-words text-base leading-relaxed text-foreground md:text-sm'

interface GeneralPageProps {
  readonly botId: string
  readonly isListView?: boolean
}

/** The General settings page for a bot: bot controls, Instructions, Auto-respond, and Usage. */
export function GeneralPage(props: GeneralPageProps) {
  const instanceKey = useBotInstanceKey(props.botId)
  const atom = generalPageAtomFor(instanceKey)
  const result = useAtomValue(atom)
  const retry = useAtomRefresh(atom)
  const isListView = props.isListView === true
  const bots = useBotsRoot()?.bots ?? []
  const botName = bots.find(entry => entry.id === props.botId)?.name ?? props.botId

  const wrap = (children: React.ReactNode) => (
    <SplitDetail
      title="General"
      isListView={isListView}
      backTo="/settings/bots/$botId"
      backParams={{ botId: props.botId }}
      backLabel={botName}
    >
      {children}
    </SplitDetail>
  )

  return matchCached(result, {
    onInitial: () => wrap(<EmptyLine>Loading…</EmptyLine>),
    onFailure: failure => wrap(<PageError cause={failure.cause} onRetry={retry} />),
    onSuccess: value => (
      <GeneralOwner.Provider input={value.general} scopeKey={instanceKey}>
        <InstructionsOwner.Provider input={value.instructions} scopeKey={instanceKey}>
          {wrap(
            <>
              <div className="space-y-8">
                <BotIdentitySection botId={props.botId} />
                <Section title="Instructions" action={<InstructionsActions />}>
                  <InstructionsEditor />
                </Section>
                <AutoRespondSection botId={props.botId} />
                <HomeSection botId={props.botId} />
                <UsageSection instanceKey={instanceKey} />
              </div>
              <UnsavedGuard />
            </>,
          )}
        </InstructionsOwner.Provider>
      </GeneralOwner.Provider>
    ),
  })
}

interface BotIdentitySectionProps {
  readonly botId: string
}

function BotIdentitySection(props: BotIdentitySectionProps) {
  const botsMachine = BotsOwner.useMachine()
  const send = useAtomSet(botsMachine.send)
  const root = useBotsRoot()
  const bot = root?.bots.find(entry => entry.id === props.botId)
  const [nameDraft, setNameDraft] = useState<string | null>(null)

  if (bot === undefined) return null

  const currentName = nameDraft ?? bot.name
  const busy = root?.busyIds.includes(bot.id) ?? false
  const dirty = currentName.trim() !== '' && currentName.trim() !== bot.name

  const handleRename = (event: FormEvent) => {
    event.preventDefault()
    const next = currentName.trim()
    if (next === '' || next === bot.name) return
    send(
      BotsEvents.Update({
        id: bot.id,
        name: next,
        enabled: null,
        botGeneration: bot.generation,
      }),
    )
    setNameDraft(null)
  }

  return (
    <Section
      title="Bot"
      action={
        bot.isDefault ? null : (
          <ConfirmDialog
            trigger={
              <Button
                variant="ghost"
                size="sm"
                disabled={busy}
                className="text-destructive hover:text-destructive"
              >
                <Trash2 data-icon="inline-start" />
                Delete bot
              </Button>
            }
            title={`Delete ${bot.name}?`}
            detail="This stops the bot and permanently removes its memory, workspace, schedules, and webhooks."
            confirmLabel="Delete"
            confirmVariant="destructive"
            onConfirm={() => send(BotsEvents.Remove({ id: bot.id, botGeneration: bot.generation }))}
          />
        )
      }
    >
      <div className="space-y-3 rounded-2xl border border-border bg-card px-4 py-3.5 shadow-xs">
        <form onSubmit={handleRename}>
          <FieldRow id={`bot-name-${bot.id}`} label="Name">
            <div className="flex items-center justify-end gap-2">
              <Input
                id={`bot-name-${bot.id}`}
                value={currentName}
                onChange={event => setNameDraft(event.target.value)}
                disabled={busy}
                className="max-w-xs"
              />
              {dirty ? (
                <Button type="submit" size="sm" disabled={busy}>
                  Save
                </Button>
              ) : null}
            </div>
          </FieldRow>
        </form>
        <FieldRow labelId={`bot-environment-${bot.id}`} label="Environment">
          <div className="flex justify-end">
            <EnvironmentSelect bot={bot} busy={busy} labelId={`bot-environment-${bot.id}`} />
          </div>
        </FieldRow>
        <div className="flex items-center justify-between gap-4 border-t border-border/60 pt-3">
          <div className="min-w-0">
            <p className="text-sm font-medium text-foreground">Enabled</p>
            <p className="mt-0.5 text-xs text-muted-foreground">
              Run this bot&apos;s Discord gateway, schedules, and webhooks.
            </p>
          </div>
          <Switch
            aria-label={`Enable ${bot.name}`}
            checked={bot.enabled}
            disabled={busy}
            onCheckedChange={enabled =>
              send(
                BotsEvents.Update({
                  id: bot.id,
                  name: null,
                  enabled,
                  botGeneration: bot.generation,
                }),
              )
            }
          />
        </div>
      </div>
    </Section>
  )
}

interface EnvironmentSelectProps {
  readonly bot: BotSummary
  readonly busy: boolean
  readonly labelId: string
}

/** Picks the execution environment a bot runs on; offline environments stay selectable. */
function EnvironmentSelect(props: EnvironmentSelectProps) {
  const machine = BotsOwner.useMachine()
  const send = useAtomSet(machine.send)
  // The list is cached across pages; refetch on open so accepts and online changes show up.
  const refreshEnvironments = useAtomRefresh(environmentsPageAtom)
  const environments = AsyncResult.getOrElse(useAtomValue(environmentsPageAtom), () => ({
    pending: [],
    environments: [],
  })).environments
  const onlineByName = new Map(environments.map(entry => [entry.name, entry.online]))
  const names = [
    ...new Set(['local', props.bot.environment, ...environments.map(entry => entry.name)]),
  ]
  const items = names.map(name => ({ value: name, label: name === 'local' ? 'Local' : name }))
  const isOnline = (name: string): boolean =>
    name === 'local' ||
    (onlineByName.get(name) ?? (name === props.bot.environment && props.bot.environmentOnline))

  return (
    <Select
      value={props.bot.environment}
      items={items}
      disabled={props.busy}
      onOpenChange={open => {
        if (open) refreshEnvironments()
      }}
      onValueChange={next => {
        if (next !== null && next !== props.bot.environment) {
          send(
            BotsEvents.SetEnvironment({
              id: props.bot.id,
              environment: next,
              botGeneration: props.bot.generation,
            }),
          )
        }
      }}
    >
      <SelectTrigger aria-labelledby={props.labelId} className="w-full max-w-xs">
        <SelectValue />
      </SelectTrigger>
      <SelectContent>
        {items.map(item => (
          <SelectItem key={item.value} value={item.value}>
            {isOnline(item.value) ? null : (
              <span aria-hidden="true" className="size-1.5 rounded-full bg-muted-foreground/40" />
            )}
            {item.label}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  )
}

interface HomeSectionProps {
  readonly botId: string
}

/** Where scheduled jobs and check-ins report with no other destination: every home set here. */
function HomeSection(props: HomeSectionProps) {
  const machine = GeneralOwner.useMachine()
  const send = useAtomSet(machine.send)
  return (
    <Section title="Home">
      <div className="space-y-3 rounded-2xl border border-border bg-card px-4 py-3.5 shadow-xs">
        <MachineState machine={machine} path="">
          {({ value, states }) => (
            <div className="flex items-center justify-between gap-4">
              <p className="text-sm font-medium text-foreground">Web chat</p>
              <Switch
                aria-label="Web chat home"
                disabled={states.WebHome.state.path === 'WebHome.Saving'}
                checked={value.webHome}
                onCheckedChange={enabled => send(GeneralEvents.SetWebHome({ enabled }))}
              />
            </div>
          )}
        </MachineState>
        <div className="border-t border-border/60 pt-3">
          <p className="mb-1 text-sm font-medium text-foreground">Discord</p>
          <DiscordSettingsOwner botId={props.botId}>
            {loadState =>
              loadState.kind === 'loaded' ? (
                <DiscordHomeFields />
              ) : loadState.kind === 'failed' ? (
                <PageError cause={loadState.cause} onRetry={loadState.onRetry} />
              ) : null
            }
          </DiscordSettingsOwner>
        </div>
      </div>
    </Section>
  )
}

interface AutoRespondSectionProps {
  readonly botId: string
}

function AutoRespondSection(props: AutoRespondSectionProps) {
  const machine = GeneralOwner.useMachine()
  const send = useAtomSet(machine.send)
  useEffect(() => {
    send(GeneralEvents.Refresh())
  }, [send])
  return (
    <MachineState machine={machine} path="">
      {({ value }) => (
        <div className="flex items-center justify-between gap-4 rounded-2xl border border-border bg-card px-4 py-3.5 shadow-xs">
          <div className="min-w-0">
            <p className="text-sm font-medium text-foreground">Auto-respond</p>
            <p className="mt-0.5 text-xs text-muted-foreground">
              Reply in channels and threads without an @mention when spoken to.
            </p>
            {value.hasUtilityModel ? null : (
              <p className="mt-1 text-xs text-muted-foreground">
                Requires a decision or utility model.{' '}
                <Link
                  to="/settings/bots/$botId/models"
                  params={{ botId: props.botId }}
                  className="font-medium text-foreground underline underline-offset-2"
                >
                  Configure in Models
                </Link>
              </p>
            )}
          </div>
          <Switch
            aria-label="Auto-respond without @mention"
            checked={value.autoRespond}
            onCheckedChange={enabled => send(GeneralEvents.SetAutoRespond({ enabled }))}
          />
        </div>
      )}
    </MachineState>
  )
}

function InstructionsActions() {
  const machine = InstructionsOwner.useMachine()
  const send = useAtomSet(machine.send)
  const canReset = AsyncResult.match(useMachineResult(resetAllowed(machine)), {
    onInitial: () => false,
    onFailure: () => false,
    onSuccess: ({ value }) => value,
  })
  return (
    <div className="flex items-center gap-1">
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
                  size="icon-sm"
                  aria-label="Reset to default"
                  disabled={!canReset}
                >
                  <Undo2 />
                </Button>
              }
              title="Reset instructions?"
              detail="This replaces your custom instructions with the default."
              confirmLabel="Reset"
              onConfirm={() => send(InstructionsEvents.Reset())}
            />
          )
        }
      </MachineState>
    </div>
  )
}

interface InstructionsFooterProps {
  readonly error: string | null
  readonly failedAction: 'save' | 'reset' | null
  readonly hasSaved: boolean
  readonly canRetrySave: boolean
  readonly isSaving: boolean
  readonly isResetting: boolean
  readonly onRetrySave: () => void
  readonly onRetryReset: () => void
}

function InstructionsFooter(props: InstructionsFooterProps) {
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
            id="instructions-error"
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

function InstructionsEditor() {
  const machine = InstructionsOwner.useMachine()
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
              'grid grid-cols-1 rounded-2xl border bg-card shadow-xs transition-colors',
              value.error === null
                ? 'border-border focus-within:border-foreground/35'
                : 'border-destructive',
            )}
          >
            <div
              aria-hidden="true"
              className={cn(textClassName, 'invisible max-h-[32rem] overflow-hidden')}
            >
              {value.draftText}{' '}
            </div>
            <textarea
              aria-label="Instructions"
              aria-invalid={value.error !== null}
              aria-describedby={value.error === null ? undefined : 'instructions-error'}
              className={cn(
                textClassName,
                'h-full max-h-[32rem] resize-none overflow-y-auto bg-transparent caret-foreground outline-none disabled:cursor-not-allowed disabled:opacity-50',
              )}
              value={value.draftText}
              disabled={isResetting}
              onChange={event => send(InstructionsEvents.Edit({ text: event.target.value }))}
              onBlur={() => send(InstructionsEvents.Flush())}
            />
          </div>
          <InstructionsFooter
            error={value.error}
            failedAction={value.failedAction}
            hasSaved={value.hasSaved}
            canRetrySave={value.draftText.trim() !== ''}
            isSaving={isSaving}
            isResetting={isResetting}
            onRetrySave={() => send(InstructionsEvents.Flush())}
            onRetryReset={() => send(InstructionsEvents.Reset())}
          />
        </div>
      )}
    </MachineState>
  )
}

function UnsavedGuard() {
  const machine = InstructionsOwner.useMachine()
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
            Your instruction edits have not finished saving yet.
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
