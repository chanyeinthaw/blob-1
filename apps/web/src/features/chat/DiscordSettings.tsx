import type { ApiError } from '@blob-1/client/api'
import type {
  DiscordAccessEntry,
  DiscordAccessList,
  DiscordAccessMode,
  DiscordAccessScope,
  DiscordConnection,
  DiscordDirectory,
  DiscordInteractionMode,
  DiscordInteractionOverride,
  DiscordInteractionScope,
  DiscordReplyMode,
  DiscordReplyOverride,
  DiscordReplyScope,
  Home,
} from '@blob-1/contracts'
import { useAtomRefresh, useAtomSet, useAtomValue } from '@effect/atom-react'
import { AtomMachine } from '@typeonce/effect-machine/reactivity'
import { MachineState, useMachineResult } from '@typeonce/effect-machine-react'
import { type Cause, Option } from 'effect'
import { AsyncResult } from 'effect/reactivity'
import { CircleHelp, Globe, Hash, KeyRound, Pencil, Plus, Server, Trash2, User } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'

import { ConfirmDialog } from '~/components/ConfirmDialog'
import { EmptyLine, FieldError, FieldRow, Section } from '~/components/layout/Page'
import { Picker } from '~/components/Picker'
import { Button } from '~/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '~/components/ui/dialog'
import { Input } from '~/components/ui/input'
import { ToggleGroup, ToggleGroupItem } from '~/components/ui/toggle-group'
import { Tooltip, TooltipContent, TooltipTrigger } from '~/components/ui/tooltip'
import { MachineAtoms } from '~/lib/runtime'
import { createPersistentMachineContext } from '~/lib/shellMachines'
import { matchCached } from '~/lib/useBackgroundRefresh'
import { cn } from '~/lib/utils'

import { DiscordEvents, type DiscordInput, DiscordMachine } from './discordMachine'
import { discordPageAtom } from './discordState'

const DiscordOwner = createPersistentMachineContext(
  'discord',
  ({ input }: { input: DiscordInput }) => MachineAtoms.make(DiscordMachine, input),
)

interface Status {
  readonly label: string
  readonly shortLabel: string
  readonly dot: string
}

export function statusOf(connection: DiscordConnection): Status {
  switch (connection.state) {
    case 'off':
      return { label: 'Not connected', shortLabel: 'Off', dot: 'bg-ring' }
    case 'connecting':
      return { label: 'Connecting…', shortLabel: 'Connecting…', dot: 'bg-ring' }
    case 'online':
      return { label: `Online as ${connection.bot}`, shortLabel: 'Online', dot: 'bg-ok' }
    case 'error':
      return { label: 'Error', shortLabel: 'Error', dot: 'bg-destructive' }
  }
}

export type DiscordLoadState =
  | { readonly kind: 'loading' }
  | { readonly kind: 'loaded' }
  | { readonly kind: 'failed'; readonly cause: Cause.Cause<ApiError>; readonly onRetry: () => void }

interface DiscordOwnerProps {
  readonly children: (state: DiscordLoadState) => React.ReactNode
}

/** Loads Discord settings and owns their machine for the subtree. */
export function DiscordSettingsOwner(props: DiscordOwnerProps) {
  const result = useAtomValue(discordPageAtom)
  const retry = useAtomRefresh(discordPageAtom)
  return matchCached(result, {
    onInitial: () => props.children({ kind: 'loading' }),
    onFailure: failure => props.children({ kind: 'failed', cause: failure.cause, onRetry: retry }),
    onSuccess: value => (
      <DiscordOwner.Provider input={value}>
        <DiscordSyncBridge />
        {props.children({ kind: 'loaded' })}
      </DiscordOwner.Provider>
    ),
  })
}

function DiscordSyncBridge() {
  const machine = DiscordOwner.useMachine()
  const send = useAtomSet(machine.send)
  useEffect(() => {
    send(DiscordEvents.Refresh())
  }, [send])
  return null
}

/** The Discord status pill shown in the detail title row. */
export function DiscordStatusBadge() {
  const machine = DiscordOwner.useMachine()
  return (
    <MachineState machine={machine} path="">
      {({ value }) => {
        const status = statusOf(value.discord.connection)
        return (
          <span
            aria-live="polite"
            className={cn(
              'inline-flex h-7 max-w-full items-center gap-2 rounded-full bg-muted px-3 text-xs font-medium',
              value.discord.connection.state === 'error'
                ? 'text-destructive'
                : 'text-muted-foreground',
            )}
          >
            <span aria-hidden="true" className={cn('size-2 shrink-0 rounded-full', status.dot)} />
            <span className="truncate">{status.label}</span>
          </span>
        )
      }}
    </MachineState>
  )
}

/** The status indicator beside Discord in the platform list. */
export function DiscordStatusDot() {
  const machine = DiscordOwner.useMachine()
  return (
    <MachineState machine={machine} path="">
      {({ value }) => {
        const status = statusOf(value.discord.connection)
        return (
          <span className="flex shrink-0 items-center gap-1.5">
            <span aria-hidden="true" className={cn('size-2 rounded-full', status.dot)} />
            <span
              aria-hidden="true"
              className={cn(
                'text-xs font-normal md:hidden',
                value.discord.connection.state === 'error'
                  ? 'text-destructive'
                  : 'text-muted-foreground',
              )}
            >
              {status.shortLabel}
            </span>
            <span className="sr-only">{status.label}</span>
          </span>
        )
      }}
    </MachineState>
  )
}

export function DiscordSettings() {
  return (
    <div>
      <TokenSection />
      <AccessSection />
      <InteractionModeSection />
      <ReplyModeSection />
      <HomeSection />
    </div>
  )
}

function TokenSection() {
  const machine = DiscordOwner.useMachine()
  const send = useAtomSet(machine.send)
  const replaceButtonRef = useRef<HTMLButtonElement>(null)
  const isSaved = AsyncResult.getOrElse(
    useMachineResult(AtomMachine.matches(machine, 'Token.Saved')),
    () => false,
  )
  const wasSaved = useRef(isSaved)
  useEffect(() => {
    if (!wasSaved.current && isSaved) {
      replaceButtonRef.current?.focus()
    }
    wasSaved.current = isSaved
  }, [isSaved])

  return (
    <Section title="Bot token">
      <MachineState machine={machine} path="Token.Saved">
        {() => (
          <div className="flex min-h-10 flex-wrap items-center gap-x-3 gap-y-2">
            <KeyRound className="size-4 shrink-0 text-muted-foreground" />
            <span className="mr-auto font-mono text-sm tracking-widest text-muted-foreground">
              ••••••••••••••••
            </span>
            <div className="flex items-center gap-1">
              <Tooltip>
                <TooltipTrigger
                  render={
                    <Button
                      ref={replaceButtonRef}
                      variant="ghost"
                      size="icon"
                      aria-label="Replace token"
                      onClick={() => send(DiscordEvents.ReplaceToken())}
                    >
                      <Pencil />
                    </Button>
                  }
                />
                <TooltipContent>Replace token</TooltipContent>
              </Tooltip>
              <ConfirmDialog
                tooltip="Remove token"
                trigger={
                  <Button
                    variant="ghost"
                    size="icon"
                    aria-label="Remove token"
                    className="text-destructive hover:bg-destructive/10 hover:text-destructive"
                  >
                    <Trash2 />
                  </Button>
                }
                title="Remove the Discord token?"
                detail="Blob-1 disconnects from Discord."
                confirmLabel="Remove"
                onConfirm={() => send(DiscordEvents.RemoveToken())}
              />
            </div>
          </div>
        )}
      </MachineState>
      <TokenFormSlot />
      <MachineState machine={machine} path="Token.Removing">
        {() => <EmptyLine>Removing…</EmptyLine>}
      </MachineState>
      <MachineState machine={machine} path="">
        {({ value }) =>
          value.discord.connection.state === 'error' ? (
            <p
              role="alert"
              className="mt-2 break-words text-xs text-destructive [overflow-wrap:anywhere]"
            >
              {value.discord.connection.message}
            </p>
          ) : null
        }
      </MachineState>
    </Section>
  )
}

/** Keeps one form mounted while editing and saving, so a rejected token stays in the field. */
function TokenFormSlot() {
  const machine = DiscordOwner.useMachine()
  const editing = AsyncResult.getOrElse(
    useMachineResult(AtomMachine.select(machine, 'Token.Editing')),
    () => Option.none(),
  )
  const isSaving = AsyncResult.getOrElse(
    useMachineResult(AtomMachine.matches(machine, 'Token.Saving')),
    () => false,
  )
  if (Option.isNone(editing) && !isSaving) return null
  return (
    <TokenForm
      error={Option.match(editing, { onNone: () => null, onSome: state => state.error })}
      isSaving={isSaving}
    />
  )
}

interface TokenFormProps {
  readonly error: string | null
  readonly isSaving: boolean
}

function TokenForm(props: TokenFormProps) {
  const machine = DiscordOwner.useMachine()
  const send = useAtomSet(machine.send)
  const [token, setToken] = useState('')
  const inputRef = useRef<HTMLInputElement>(null)
  const isReplacing = AsyncResult.match(useMachineResult(AtomMachine.select(machine, '')), {
    onInitial: () => false,
    onFailure: () => false,
    onSuccess: ({ value }) =>
      Option.match(value, {
        onNone: () => false,
        onSome: root => root.discord.token === 'set',
      }),
  })
  useEffect(() => {
    if (isReplacing) inputRef.current?.focus()
  }, [isReplacing])
  return (
    <MachineState machine={machine} path="">
      {({ value }) => (
        <form
          noValidate
          onSubmit={event => {
            event.preventDefault()
            send(DiscordEvents.SubmitToken({ token }))
          }}
        >
          <label htmlFor="discord-token" className="sr-only">
            Bot token
          </label>
          <div className="flex flex-wrap items-start gap-2">
            <div className="min-w-0 flex-1 basis-64">
              <Input
                ref={inputRef}
                id="discord-token"
                type="password"
                autoComplete="off"
                placeholder="Bot token"
                className="font-mono"
                value={token}
                disabled={props.isSaving}
                aria-invalid={props.error !== null}
                aria-describedby={props.error === null ? undefined : 'discord-token-error'}
                onChange={event => {
                  setToken(event.target.value)
                  if (props.error !== null) send(DiscordEvents.ClearTokenError())
                }}
              />
              {props.error === null ? null : (
                <FieldError id="discord-token-error">{props.error}</FieldError>
              )}
            </div>
            <div className="flex items-center gap-2">
              {value.discord.token === 'set' ? (
                <Button
                  type="button"
                  variant="ghost"
                  disabled={props.isSaving}
                  onClick={() => send(DiscordEvents.CancelToken())}
                >
                  Cancel
                </Button>
              ) : null}
              <Button type="submit" disabled={props.isSaving || token.trim() === ''}>
                {props.isSaving ? 'Connecting…' : 'Save'}
              </Button>
            </div>
          </div>
        </form>
      )}
    </MachineState>
  )
}

const SNOWFLAKE = /^\d{17,20}$/

function personName(directory: DiscordDirectory, user: string) {
  return directory.people.find(person => person.id === user)?.name
}

function initials(name: string) {
  return name
    .split(/\s+/)
    .filter(word => word !== '')
    .slice(0, 2)
    .map(word => word.charAt(0).toUpperCase())
    .join('')
}

const accessModes: ReadonlyArray<{ readonly mode: DiscordAccessMode; readonly label: string }> = [
  { mode: 'whitelist', label: 'Whitelist' },
  { mode: 'blacklist', label: 'Blacklist' },
  { mode: 'allow-all', label: 'Allow all' },
]

const accessScopes: ReadonlyArray<{ readonly scope: DiscordAccessScope; readonly label: string }> =
  [
    { scope: 'global', label: 'Global' },
    { scope: 'guild', label: 'Server' },
    { scope: 'channel', label: 'Channel' },
  ]

function isAccessMode(value: string | undefined): value is DiscordAccessMode {
  return value === 'allow-all' || value === 'whitelist' || value === 'blacklist'
}

function isAccessScope(value: string | undefined): value is DiscordAccessScope {
  return value === 'global' || value === 'guild' || value === 'channel'
}

function describeAccessScope(entry: DiscordAccessEntry, directory: DiscordDirectory): string {
  if (entry.scope === 'global') return 'Global'
  if (entry.scope === 'guild') {
    const guild = directory.guilds.find(item => item.id === entry.scopeId)
    return guild?.name ?? `server ${entry.scopeId ?? ''}`
  }
  const channel = directory.channels.find(item => item.id === entry.scopeId)
  if (channel === undefined) return `channel ${entry.scopeId ?? ''}`
  return channel.server === '' ? `#${channel.name}` : `#${channel.name} · ${channel.server}`
}

function AccessSection() {
  const machine = DiscordOwner.useMachine()
  const send = useAtomSet(machine.send)
  const isIdle = AsyncResult.getOrElse(
    useMachineResult(AtomMachine.matches(machine, 'Access.Idle')),
    () => false,
  )
  return (
    <MachineState machine={machine} path="">
      {({ value, states }) => {
        const accessState = states.Access.state
        const error = accessState.path === 'Access.Idle' ? accessState.value.error : null
        const mode = value.discord.accessMode
        const entries =
          mode === 'allow-all'
            ? []
            : value.discord.accessEntries.filter(entry => entry.list === mode)
        const addButton =
          mode === 'allow-all' ? null : (
            <Button
              size="sm"
              variant="secondary"
              disabled={!isIdle}
              onClick={() => send(DiscordEvents.OpenAccessDialog())}
            >
              <Plus data-icon="inline-start" className="size-3.5" />
              {mode === 'whitelist' ? 'Allow' : 'Block'}
            </Button>
          )
        return (
          <Section title="Access" action={addButton}>
            <div className="mb-2">
              <ToggleGroup
                aria-label="Access mode"
                spacing={1}
                value={[mode]}
                onValueChange={values => {
                  const next = values[0]
                  if (isAccessMode(next) && next !== mode) {
                    send(DiscordEvents.SetAccessMode({ mode: next }))
                  }
                }}
                className="h-10 rounded-full bg-muted p-[3px] md:h-8"
              >
                {accessModes.map(item => (
                  <ToggleGroupItem
                    key={item.mode}
                    value={item.mode}
                    className={modeToggleItemClassName}
                  >
                    {item.label}
                  </ToggleGroupItem>
                ))}
              </ToggleGroup>
            </div>
            {mode === 'allow-all' ? (
              <EmptyLine>Anyone can talk to Blob-1.</EmptyLine>
            ) : entries.length === 0 ? (
              <EmptyLine>
                {mode === 'whitelist' ? 'Nobody is whitelisted yet.' : 'Nobody is blacklisted.'}
              </EmptyLine>
            ) : (
              <ul className="flex flex-col divide-y">
                {entries.map(entry => (
                  <AccessRow
                    key={`${entry.list}:${entry.scope}:${entry.scopeId ?? ''}:${entry.userId}`}
                    entry={entry}
                    directory={value.directory}
                    disabled={!isIdle}
                  />
                ))}
              </ul>
            )}
            {!value.accessDialogOpen && error !== null ? (
              <FieldError id="discord-access-error">{error}</FieldError>
            ) : null}
            {mode === 'allow-all' ? null : (
              <AccessAddDialog
                list={mode}
                open={value.accessDialogOpen}
                isIdle={isIdle}
                error={error}
              />
            )}
          </Section>
        )
      }}
    </MachineState>
  )
}

interface AccessRowProps {
  readonly entry: DiscordAccessEntry
  readonly directory: DiscordDirectory
  readonly disabled: boolean
}

function AccessRow(props: AccessRowProps) {
  const machine = DiscordOwner.useMachine()
  const send = useAtomSet(machine.send)
  const { entry, directory, disabled } = props
  const name = personName(directory, entry.userId)
  const scopeLabel = describeAccessScope(entry, directory)
  const listName = entry.list === 'whitelist' ? 'whitelist' : 'blacklist'
  const label = name ?? entry.userId
  const detail =
    name === undefined
      ? `${entry.userId} · ${scopeLabel}`
      : `${name} · ${scopeLabel} (${entry.userId})`
  return (
    <li className="flex min-h-12 items-center gap-3 py-2">
      <span
        aria-hidden="true"
        className="grid size-7 shrink-0 place-items-center rounded-full bg-muted text-[11px] font-semibold text-muted-foreground"
      >
        {name === undefined ? <User className="size-3.5" /> : initials(name)}
      </span>
      <div className="flex min-w-0 flex-1 flex-wrap items-baseline gap-x-2 gap-y-0.5">
        <span className="break-words text-sm [overflow-wrap:anywhere]">{label}</span>
        <span className="break-words text-xs text-muted-foreground [overflow-wrap:anywhere]">
          {scopeLabel}
        </span>
      </div>
      <ConfirmDialog
        tooltip={`Remove from ${listName}`}
        trigger={
          <Button
            variant="ghost"
            size="icon"
            aria-label={`Remove ${label} (${scopeLabel}) from ${listName}`}
            disabled={disabled}
            className="text-destructive hover:bg-destructive/10 hover:text-destructive"
          >
            <Trash2 />
          </Button>
        }
        title={`Remove from ${listName}?`}
        detail={detail}
        confirmLabel="Remove"
        onConfirm={() => send(DiscordEvents.RemoveAccessEntry(entry))}
      />
    </li>
  )
}

interface AccessAddDialogProps {
  readonly list: DiscordAccessList
  readonly open: boolean
  readonly isIdle: boolean
  readonly error: string | null
}

function accessGuildFilter(
  scope: DiscordAccessScope,
  scopeId: string,
  directory: DiscordDirectory,
): string | null {
  if (scope === 'guild') return scopeId === '' ? null : scopeId
  if (scope === 'channel') {
    const guildId = directory.channels.find(channel => channel.id === scopeId)?.guildId ?? ''
    return guildId === '' ? null : guildId
  }
  return null
}

function AccessScopePicker(props: {
  readonly scope: DiscordAccessScope
  readonly scopeId: string
  readonly directory: DiscordDirectory
  readonly disabled: boolean
  readonly onChange: (scopeId: string) => void
}) {
  if (props.scope === 'global') return null
  const isGuild = props.scope === 'guild'
  const options = isGuild
    ? props.directory.guilds.map(guild => ({
        value: guild.id,
        label: guild.name,
        hint: guild.id,
      }))
    : props.directory.channels
        .toSorted((a, b) => a.server.localeCompare(b.server) || a.name.localeCompare(b.name))
        .map(channel => ({
          value: channel.id,
          label: `#${channel.name}`,
          hint: null,
          group: channel.server,
        }))
  return (
    <FieldRow id="discord-access-scope-target" label={isGuild ? 'Server' : 'Channel'}>
      <Picker
        id="discord-access-scope-target"
        placeholder={isGuild ? 'Choose a server' : 'Choose a channel'}
        value={props.scopeId}
        disabled={props.disabled}
        clearOnEdit
        options={options}
        customOption={raw => {
          const id = raw.trim()
          if (!SNOWFLAKE.test(id)) return null
          return { value: id, label: `Use ID ${id}`, hint: null }
        }}
        emptyMessage={raw => {
          const trimmed = raw.trim()
          if (/^\d+$/.test(trimmed) && !SNOWFLAKE.test(trimmed)) {
            return 'Discord IDs have 17–20 digits'
          }
          return 'No matches'
        }}
        onChange={props.onChange}
      />
    </FieldRow>
  )
}

/** Dialog to add a global, server-scoped, or channel-scoped whitelist/blacklist entry. */
function AccessAddDialog(props: AccessAddDialogProps) {
  const machine = DiscordOwner.useMachine()
  const send = useAtomSet(machine.send)
  return (
    <Dialog
      open={props.open}
      onOpenChange={open => {
        if (!open && props.isIdle) send(DiscordEvents.CloseAccessDialog())
      }}
    >
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>
            {props.list === 'whitelist' ? 'Allow a person' : 'Block a person'}
          </DialogTitle>
        </DialogHeader>
        <MachineState machine={machine} path="">
          {({ value }) => {
            const scope = value.accessScope
            const selectedScopeId = scope === 'global' ? null : value.accessScopeId
            const exists = (userId: string) =>
              value.discord.accessEntries.some(
                entry =>
                  entry.list === props.list &&
                  entry.scope === scope &&
                  entry.scopeId === selectedScopeId &&
                  entry.userId === userId,
              )
            const guildFilter = accessGuildFilter(scope, value.accessScopeId, value.directory)
            const peopleOptions = value.directory.people
              .filter(
                person =>
                  !exists(person.id) &&
                  (guildFilter === null || person.guilds.includes(guildFilter)),
              )
              .map(person => ({ value: person.id, label: person.name, hint: person.id }))
            const scopeValid = scope === 'global' || SNOWFLAKE.test(value.accessScopeId)
            const canSubmit =
              props.isIdle &&
              scopeValid &&
              SNOWFLAKE.test(value.accessUser) &&
              !exists(value.accessUser)
            const verb = props.list === 'whitelist' ? 'Allow' : 'Block'
            return (
              <div className="space-y-1">
                <FieldRow labelId="discord-access-scope-label" label="Scope">
                  <ToggleGroup
                    aria-labelledby="discord-access-scope-label"
                    spacing={1}
                    disabled={!props.isIdle}
                    value={[scope]}
                    onValueChange={values => {
                      const next = values[0]
                      if (isAccessScope(next) && next !== scope) {
                        send(DiscordEvents.SelectAccessScope({ scope: next }))
                      }
                    }}
                    className="h-10 rounded-full bg-muted p-[3px] md:h-8"
                  >
                    {accessScopes.map(item => (
                      <ToggleGroupItem
                        key={item.scope}
                        value={item.scope}
                        className={modeToggleItemClassName}
                      >
                        {item.label}
                      </ToggleGroupItem>
                    ))}
                  </ToggleGroup>
                </FieldRow>
                <AccessScopePicker
                  scope={scope}
                  scopeId={value.accessScopeId}
                  directory={value.directory}
                  disabled={!props.isIdle}
                  onChange={scopeId => send(DiscordEvents.SelectAccessScopeId({ scopeId }))}
                />
                <FieldRow id="discord-access-user" label="Person">
                  <Picker
                    id="discord-access-user"
                    placeholder="Choose a person or paste an ID"
                    value={value.accessUser}
                    disabled={!props.isIdle}
                    clearOnEdit
                    options={peopleOptions}
                    customOption={raw => {
                      const id = raw.trim()
                      if (!SNOWFLAKE.test(id) || exists(id)) return null
                      return { value: id, label: `Use ID ${id}`, hint: null }
                    }}
                    emptyMessage={raw => {
                      const trimmed = raw.trim()
                      if (trimmed === '') return 'No matches'
                      if (exists(trimmed)) {
                        return props.list === 'whitelist'
                          ? 'Already whitelisted'
                          : 'Already blacklisted'
                      }
                      if (/^\d+$/.test(trimmed) && !SNOWFLAKE.test(trimmed)) {
                        return 'Discord user IDs have 17–20 digits'
                      }
                      return 'No matches'
                    }}
                    onChange={userId => send(DiscordEvents.SelectAccessUser({ userId }))}
                  />
                </FieldRow>
                {props.error === null ? null : (
                  <FieldError id="discord-access-dialog-error">{props.error}</FieldError>
                )}
                <DialogFooter className="mt-4">
                  <Button
                    type="button"
                    variant="ghost"
                    disabled={!props.isIdle}
                    onClick={() => send(DiscordEvents.CloseAccessDialog())}
                  >
                    Cancel
                  </Button>
                  <Button
                    type="button"
                    disabled={!canSubmit}
                    onClick={() =>
                      send(
                        DiscordEvents.AddAccessEntry({
                          list: props.list,
                          scope,
                          scopeId: selectedScopeId,
                          userId: value.accessUser,
                        }),
                      )
                    }
                  >
                    {props.isIdle ? verb : 'Saving…'}
                  </Button>
                </DialogFooter>
              </div>
            )
          }}
        </MachineState>
      </DialogContent>
    </Dialog>
  )
}

const modeToggleItemClassName =
  'h-full rounded-full px-3 text-muted-foreground hover:bg-transparent hover:text-foreground aria-pressed:bg-background aria-pressed:text-foreground aria-pressed:shadow-sm'

interface DefaultModeRowProps {
  readonly labelId: string
  /** Who the default applies to, e.g. "Everyone else". */
  readonly label: string
  /** Short muted note after the label, e.g. why Auto cannot take effect. */
  readonly status: string | null
  readonly children: React.ReactNode
}

/** The Default row of a mode list, laid out like the override rows below it. */
function DefaultModeRow(props: DefaultModeRowProps) {
  return (
    <li className="flex min-h-12 flex-wrap items-center gap-x-3 gap-y-2 py-2">
      <span
        aria-hidden="true"
        className="grid size-7 shrink-0 place-items-center rounded-full bg-muted text-muted-foreground"
      >
        <Globe className="size-3.5" />
      </span>
      <div className="flex min-w-0 flex-1 basis-40 flex-wrap items-baseline gap-x-2 gap-y-0.5">
        <span id={props.labelId} className="text-sm">
          {props.label}
        </span>
        {props.status === null ? null : (
          <span className="text-xs text-muted-foreground">{props.status}</span>
        )}
      </div>
      <div className="ml-auto flex items-center gap-1">
        {props.children}
        <span aria-hidden="true" className="size-10 shrink-0 md:size-8" />
      </div>
    </li>
  )
}

interface ModeHelpItem {
  readonly term: string
  readonly description: string
}

const interactionModeHelp: ReadonlyArray<ModeHelpItem> = [
  { term: 'Mention only', description: 'replies only when mentioned, replied to, or in DMs.' },
  {
    term: 'Always',
    description: 'every message goes to the main model, which may stay silent.',
  },
  {
    term: 'Auto',
    description:
      'the utility model decides whether to reply; needs a utility model and Auto-respond on, otherwise acts like Mention only.',
  },
]

const replyModeHelp: ReadonlyArray<ModeHelpItem> = [
  {
    term: 'Auto',
    description:
      "the utility model picks an inline reply or a thread (thread if it can't decide or there is no utility model).",
  },
  { term: 'Thread', description: 'always replies in a new thread.' },
  { term: 'Direct', description: 'always replies inline, never starts threads.' },
]

interface ModeHelpProps {
  /** Accessible name of the trigger, e.g. "About interaction modes". */
  readonly label: string
  readonly items: ReadonlyArray<ModeHelpItem>
}

/** A circled-? button whose tooltip explains each mode option in one line. */
function ModeHelp(props: ModeHelpProps) {
  return (
    <Tooltip>
      <TooltipTrigger
        render={
          <Button
            type="button"
            variant="ghost"
            size="icon-xs"
            aria-label={props.label}
            className="text-muted-foreground"
          >
            <CircleHelp className="size-3.5" />
          </Button>
        }
      />
      <TooltipContent className="flex-col items-start">
        {props.items.map(item => (
          <p key={item.term}>
            <span className="font-semibold">{item.term}</span>
            {' – '}
            {item.description}
          </p>
        ))}
      </TooltipContent>
    </Tooltip>
  )
}

const interactionModes: ReadonlyArray<{
  readonly mode: DiscordInteractionMode
  readonly label: string
}> = [
  { mode: 'mention-only', label: 'Mention only' },
  { mode: 'always', label: 'Always' },
  { mode: 'auto', label: 'Auto' },
]

const modeScopes: ReadonlyArray<{
  readonly scope: DiscordInteractionScope
  readonly label: string
}> = [
  { scope: 'channel', label: 'Channel' },
  { scope: 'user', label: 'Person' },
  { scope: 'server', label: 'Server' },
]

function isInteractionMode(value: string | undefined): value is DiscordInteractionMode {
  return value === 'mention-only' || value === 'always' || value === 'auto'
}

function isInteractionScope(value: string | undefined): value is DiscordInteractionScope {
  return value === 'channel' || value === 'user' || value === 'server'
}

/** Per-channel, per-person and per-server overrides of the default interaction gate. */
function InteractionModeSection() {
  const machine = DiscordOwner.useMachine()
  const send = useAtomSet(machine.send)
  const isIdle = AsyncResult.getOrElse(
    useMachineResult(AtomMachine.matches(machine, 'Interaction.Idle')),
    () => false,
  )
  return (
    <Section
      title="Interaction mode"
      titleExtra={<ModeHelp label="About interaction modes" items={interactionModeHelp} />}
      action={
        <Button
          size="sm"
          variant="secondary"
          disabled={!isIdle}
          onClick={() => send(DiscordEvents.OpenModeDialog())}
        >
          <Plus data-icon="inline-start" className="size-3.5" />
          Add
        </Button>
      }
    >
      <MachineState machine={machine} path="">
        {({ value, states }) => {
          const interaction = states.Interaction.state
          const error = interaction.path === 'Interaction.Idle' ? interaction.value.error : null
          const autoInactive =
            value.discord.defaultInteractionMode === 'auto' &&
            (!value.general.autoRespond || !value.general.hasUtilityModel)
          return (
            <>
              <ul className="flex flex-col divide-y">
                <DefaultModeRow
                  labelId="discord-default-mode-label"
                  label="Everyone else"
                  status={
                    autoInactive
                      ? value.general.autoRespond
                        ? 'Inactive: no utility model'
                        : 'Inactive: Auto-respond is off'
                      : null
                  }
                >
                  <ToggleGroup
                    aria-labelledby="discord-default-mode-label"
                    spacing={1}
                    disabled={!isIdle}
                    value={[value.discord.defaultInteractionMode]}
                    onValueChange={values => {
                      const next = values[0]
                      if (
                        isInteractionMode(next) &&
                        next !== value.discord.defaultInteractionMode
                      ) {
                        send(DiscordEvents.SetDefaultInteractionMode({ mode: next }))
                      }
                    }}
                    className="h-10 rounded-full bg-muted p-[3px] md:h-8"
                  >
                    {interactionModes.map(item => (
                      <ToggleGroupItem
                        key={item.mode}
                        value={item.mode}
                        className={modeToggleItemClassName}
                      >
                        {item.label}
                      </ToggleGroupItem>
                    ))}
                  </ToggleGroup>
                </DefaultModeRow>
                {value.discord.interactionModes.map(override => (
                  <OverrideRow
                    key={`${override.scope}:${override.guildId ?? ''}:${override.targetId}`}
                    override={override}
                    directory={value.directory}
                    disabled={!isIdle}
                  />
                ))}
              </ul>
              {!value.modeDialogOpen && error !== null ? (
                <FieldError id="discord-mode-error">{error}</FieldError>
              ) : null}
              <ModeAddDialog open={value.modeDialogOpen} isIdle={isIdle} error={error} />
            </>
          )
        }}
      </MachineState>
    </Section>
  )
}

interface OverrideRowProps {
  readonly override: DiscordInteractionOverride
  readonly directory: DiscordDirectory
  readonly disabled: boolean
}

interface OverrideTargetKey {
  readonly scope: DiscordInteractionScope
  readonly guildId: string | null
  readonly targetId: string
}

function resolveUserOverrideTarget(override: OverrideTargetKey, directory: DiscordDirectory) {
  const name = directory.people.find(person => person.id === override.targetId)?.name
  const guildLabel =
    override.guildId === null
      ? 'Unassigned server (inactive)'
      : (directory.guilds.find(guild => guild.id === override.guildId)?.name ??
        `server ${override.guildId}`)
  return { name, label: name ?? override.targetId, guildLabel }
}

function resolveOverrideTarget(override: OverrideTargetKey, directory: DiscordDirectory) {
  if (override.scope === 'channel') {
    const channel = directory.channels.find(entry => entry.id === override.targetId)
    const name = channel?.name
    return {
      name,
      label: name === undefined ? override.targetId : `#${name}`,
      guildLabel: channel?.server ?? null,
    }
  }
  if (override.scope === 'server') {
    const name = directory.guilds.find(guild => guild.id === override.targetId)?.name
    return { name, label: name ?? `server ${override.targetId}`, guildLabel: 'Server' }
  }
  return resolveUserOverrideTarget(override, directory)
}

function describeOverride(override: OverrideTargetKey, directory: DiscordDirectory) {
  const { name, label, guildLabel } = resolveOverrideTarget(override, directory)
  const detail = [label, guildLabel].filter(Boolean).join(' · ')
  return {
    name,
    label,
    guildLabel,
    accessibleTarget:
      override.scope === 'server'
        ? `server ${name ?? override.targetId}`
        : guildLabel === null
          ? label
          : `${label} in ${guildLabel}`,
    confirmDetail: name === undefined ? detail : `${detail} (${override.targetId})`,
  }
}

function OverrideRow(props: OverrideRowProps) {
  const machine = DiscordOwner.useMachine()
  const send = useAtomSet(machine.send)
  const { scope, guildId, targetId, mode } = props.override
  const { name, label, guildLabel, accessibleTarget, confirmDetail } = describeOverride(
    props.override,
    props.directory,
  )
  return (
    <li className="flex min-h-12 flex-wrap items-center gap-x-3 gap-y-2 py-2">
      <span
        aria-hidden="true"
        className="grid size-7 shrink-0 place-items-center rounded-full bg-muted text-[11px] font-semibold text-muted-foreground"
      >
        {scope === 'channel' ? (
          <Hash className="size-3.5" />
        ) : scope === 'server' ? (
          <Server className="size-3.5" />
        ) : name === undefined ? (
          <User className="size-3.5" />
        ) : (
          initials(name)
        )}
      </span>
      <div className="flex min-w-0 flex-1 basis-40 flex-wrap items-baseline gap-x-2 gap-y-0.5">
        <span className="break-words text-sm [overflow-wrap:anywhere]">{label}</span>
        {guildLabel === null ? null : (
          <span className="break-words text-xs text-muted-foreground [overflow-wrap:anywhere]">
            {guildLabel}
          </span>
        )}
      </div>
      <div className="ml-auto flex items-center gap-1">
        <ToggleGroup
          aria-label={`Interaction mode for ${accessibleTarget}`}
          spacing={1}
          disabled={props.disabled || (scope === 'user' && guildId === null)}
          value={[mode]}
          onValueChange={values => {
            const next = values[0]
            if (isInteractionMode(next) && next !== mode) {
              send(DiscordEvents.SetInteractionMode({ scope, guildId, targetId, mode: next }))
            }
          }}
          className="h-10 rounded-full bg-muted p-[3px] md:h-8"
        >
          {interactionModes.map(item => (
            <ToggleGroupItem key={item.mode} value={item.mode} className={modeToggleItemClassName}>
              {item.label}
            </ToggleGroupItem>
          ))}
        </ToggleGroup>
        <ConfirmDialog
          tooltip="Remove override"
          trigger={
            <Button
              variant="ghost"
              size="icon"
              aria-label={`Remove override for ${accessibleTarget}`}
              disabled={props.disabled}
              className="text-destructive hover:bg-destructive/10 hover:text-destructive"
            >
              <Trash2 />
            </Button>
          }
          title="Remove interaction override?"
          detail={confirmDetail}
          confirmLabel="Remove"
          onConfirm={() => send(DiscordEvents.ClearInteractionMode({ scope, guildId, targetId }))}
        />
      </div>
    </li>
  )
}

interface ModeAddDialogProps {
  readonly open: boolean
  readonly isIdle: boolean
  readonly error: string | null
}

const modeTargetField = {
  channel: { label: 'Channel', placeholder: 'Choose a channel' },
  user: { label: 'Person', placeholder: 'Choose a person' },
  server: { label: 'Server', placeholder: 'Choose a server' },
} satisfies Record<DiscordInteractionScope, { label: string; placeholder: string }>

const modeTargetOptions = (
  scope: DiscordInteractionScope,
  selectedGuild: string | null,
  directory: DiscordDirectory,
  overridden: (targetId: string) => boolean,
) => {
  if (scope === 'channel') {
    return directory.channels
      .filter(channel => !overridden(channel.id))
      .toSorted((a, b) => a.server.localeCompare(b.server) || a.name.localeCompare(b.name))
      .map(channel => ({
        value: channel.id,
        label: `#${channel.name}`,
        hint: null,
        group: channel.server,
      }))
  }
  if (scope === 'server') {
    return directory.guilds
      .filter(guild => !overridden(guild.id))
      .map(guild => ({ value: guild.id, label: guild.name, hint: guild.id }))
  }
  return directory.people
    .filter(
      person =>
        !overridden(person.id) &&
        (selectedGuild === null || selectedGuild === '' || person.guilds.includes(selectedGuild)),
    )
    .map(person => ({ value: person.id, label: person.name, hint: person.id }))
}

/** Dialog to add a channel, person, or whole-server interaction mode override. */
function ModeAddDialog(props: ModeAddDialogProps) {
  const machine = DiscordOwner.useMachine()
  const send = useAtomSet(machine.send)
  return (
    <Dialog
      open={props.open}
      onOpenChange={open => {
        if (!open && props.isIdle) send(DiscordEvents.CloseModeDialog())
      }}
    >
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Add interaction override</DialogTitle>
        </DialogHeader>
        <MachineState machine={machine} path="">
          {({ value }) => {
            const scope = value.modeScope
            const selectedGuild = scope === 'user' ? value.modeGuild : null
            const overridden = (targetId: string) =>
              value.discord.interactionModes.some(
                entry =>
                  entry.scope === scope &&
                  entry.guildId === selectedGuild &&
                  entry.targetId === targetId,
              )
            const guildOptions = value.directory.guilds.map(guild => ({
              value: guild.id,
              label: guild.name,
              hint: guild.id,
            }))
            const options = modeTargetOptions(scope, selectedGuild, value.directory, overridden)
            const guildValid = scope !== 'user' || SNOWFLAKE.test(value.modeGuild)
            const targetValid =
              guildValid && SNOWFLAKE.test(value.modeTarget) && !overridden(value.modeTarget)
            return (
              <div className="space-y-1">
                <FieldRow labelId="discord-mode-scope-label" label="For">
                  <ToggleGroup
                    aria-labelledby="discord-mode-scope-label"
                    spacing={1}
                    disabled={!props.isIdle}
                    value={[scope]}
                    onValueChange={values => {
                      const next = values[0]
                      if (isInteractionScope(next) && next !== scope) {
                        send(DiscordEvents.SelectModeScope({ scope: next }))
                      }
                    }}
                    className="h-10 rounded-full bg-muted p-[3px] md:h-8"
                  >
                    {modeScopes.map(item => (
                      <ToggleGroupItem
                        key={item.scope}
                        value={item.scope}
                        className={modeToggleItemClassName}
                      >
                        {item.label}
                      </ToggleGroupItem>
                    ))}
                  </ToggleGroup>
                </FieldRow>
                {scope === 'user' ? (
                  <FieldRow id="discord-mode-guild" label="Server">
                    <Picker
                      id="discord-mode-guild"
                      placeholder="Choose a server"
                      value={value.modeGuild}
                      disabled={!props.isIdle}
                      clearOnEdit
                      options={guildOptions}
                      customOption={raw => {
                        const id = raw.trim()
                        if (!SNOWFLAKE.test(id)) return null
                        return { value: id, label: `Use ID ${id}`, hint: null }
                      }}
                      emptyMessage={raw => {
                        const trimmed = raw.trim()
                        if (/^\d+$/.test(trimmed) && !SNOWFLAKE.test(trimmed)) {
                          return 'Discord IDs have 17–20 digits'
                        }
                        return 'No matches'
                      }}
                      onChange={guildId => send(DiscordEvents.SelectModeGuild({ guildId }))}
                    />
                  </FieldRow>
                ) : null}
                <FieldRow id="discord-mode-target" label={modeTargetField[scope].label}>
                  <Picker
                    id="discord-mode-target"
                    placeholder={modeTargetField[scope].placeholder}
                    value={value.modeTarget}
                    disabled={!props.isIdle}
                    clearOnEdit
                    options={options}
                    customOption={raw => {
                      const id = raw.trim()
                      if (!SNOWFLAKE.test(id) || overridden(id)) return null
                      return { value: id, label: `Use ID ${id}`, hint: null }
                    }}
                    emptyMessage={raw => {
                      const trimmed = raw.trim()
                      if (trimmed === '') return 'No matches'
                      if (overridden(trimmed)) return 'Already has an override'
                      if (/^\d+$/.test(trimmed) && !SNOWFLAKE.test(trimmed)) {
                        return 'Discord IDs have 17–20 digits'
                      }
                      return 'No matches'
                    }}
                    onChange={targetId => send(DiscordEvents.SelectModeTarget({ targetId }))}
                  />
                </FieldRow>
                <FieldRow
                  labelId="discord-mode-setting-label"
                  label="Mode"
                  labelExtra={
                    <ModeHelp label="About interaction modes" items={interactionModeHelp} />
                  }
                >
                  <ToggleGroup
                    aria-labelledby="discord-mode-setting-label"
                    spacing={1}
                    disabled={!props.isIdle}
                    value={[value.modeSetting]}
                    onValueChange={values => {
                      const next = values[0]
                      if (isInteractionMode(next) && next !== value.modeSetting) {
                        send(DiscordEvents.SelectModeSetting({ mode: next }))
                      }
                    }}
                    className="h-10 rounded-full bg-muted p-[3px] md:h-8"
                  >
                    {interactionModes.map(item => (
                      <ToggleGroupItem
                        key={item.mode}
                        value={item.mode}
                        className={modeToggleItemClassName}
                      >
                        {item.label}
                      </ToggleGroupItem>
                    ))}
                  </ToggleGroup>
                </FieldRow>
                {props.error === null ? null : (
                  <FieldError id="discord-mode-dialog-error">{props.error}</FieldError>
                )}
                <DialogFooter className="mt-4">
                  <Button
                    type="button"
                    variant="ghost"
                    disabled={!props.isIdle}
                    onClick={() => send(DiscordEvents.CloseModeDialog())}
                  >
                    Cancel
                  </Button>
                  <Button
                    type="button"
                    disabled={!props.isIdle || !targetValid}
                    onClick={() =>
                      send(
                        DiscordEvents.SetInteractionMode({
                          scope,
                          guildId: selectedGuild,
                          targetId: value.modeTarget,
                          mode: value.modeSetting,
                        }),
                      )
                    }
                  >
                    {props.isIdle ? 'Add' : 'Saving…'}
                  </Button>
                </DialogFooter>
              </div>
            )
          }}
        </MachineState>
      </DialogContent>
    </Dialog>
  )
}

const replyModes: ReadonlyArray<{ readonly mode: DiscordReplyMode; readonly label: string }> = [
  { mode: 'auto', label: 'Auto' },
  { mode: 'thread', label: 'Thread' },
  { mode: 'direct', label: 'Direct' },
]

const replyScopes: ReadonlyArray<{ readonly scope: DiscordReplyScope; readonly label: string }> = [
  { scope: 'channel', label: 'Channel' },
  { scope: 'server', label: 'Server' },
]

const replyTargetField = {
  channel: { label: 'Channel', placeholder: 'Choose a channel' },
  server: { label: 'Server', placeholder: 'Choose a server' },
} satisfies Record<DiscordReplyScope, { label: string; placeholder: string }>

function isReplyMode(value: string | undefined): value is DiscordReplyMode {
  return value === 'auto' || value === 'thread' || value === 'direct'
}

function isReplyScope(value: string | undefined): value is DiscordReplyScope {
  return value === 'channel' || value === 'server'
}

function ReplyModeSection() {
  const machine = DiscordOwner.useMachine()
  const send = useAtomSet(machine.send)
  const isIdle = AsyncResult.getOrElse(
    useMachineResult(AtomMachine.matches(machine, 'ReplyMode.Idle')),
    () => false,
  )
  return (
    <Section
      title="Reply mode"
      titleExtra={<ModeHelp label="About reply modes" items={replyModeHelp} />}
      action={
        <Button
          size="sm"
          variant="secondary"
          disabled={!isIdle}
          onClick={() => send(DiscordEvents.OpenReplyDialog())}
        >
          <Plus data-icon="inline-start" className="size-3.5" />
          Add
        </Button>
      }
    >
      <MachineState machine={machine} path="">
        {({ value, states }) => {
          const reply = states.ReplyMode.state
          const error = reply.path === 'ReplyMode.Idle' ? reply.value.error : null
          return (
            <>
              <ul className="flex flex-col divide-y">
                <DefaultModeRow
                  labelId="discord-default-reply-label"
                  label="All channels"
                  status={null}
                >
                  <ToggleGroup
                    aria-labelledby="discord-default-reply-label"
                    spacing={1}
                    disabled={!isIdle}
                    value={[value.discord.defaultReplyMode]}
                    onValueChange={values => {
                      const next = values[0]
                      if (isReplyMode(next) && next !== value.discord.defaultReplyMode) {
                        send(DiscordEvents.SetDefaultReplyMode({ mode: next }))
                      }
                    }}
                    className="h-10 rounded-full bg-muted p-[3px] md:h-8"
                  >
                    {replyModes.map(item => (
                      <ToggleGroupItem
                        key={item.mode}
                        value={item.mode}
                        className={modeToggleItemClassName}
                      >
                        {item.label}
                      </ToggleGroupItem>
                    ))}
                  </ToggleGroup>
                </DefaultModeRow>
                {value.discord.replyModes.map(override => (
                  <ReplyOverrideRow
                    key={`${override.scope}:${override.targetId}`}
                    override={override}
                    directory={value.directory}
                    disabled={!isIdle}
                  />
                ))}
              </ul>
              {!value.replyDialogOpen && error !== null ? (
                <FieldError id="discord-reply-error">{error}</FieldError>
              ) : null}
              <ReplyAddDialog open={value.replyDialogOpen} isIdle={isIdle} error={error} />
            </>
          )
        }}
      </MachineState>
    </Section>
  )
}

interface ReplyOverrideRowProps {
  readonly override: DiscordReplyOverride
  readonly directory: DiscordDirectory
  readonly disabled: boolean
}

function describeReplyOverride(override: DiscordReplyOverride, directory: DiscordDirectory) {
  return describeOverride(
    { scope: override.scope, guildId: null, targetId: override.targetId },
    directory,
  )
}

function ReplyOverrideRow(props: ReplyOverrideRowProps) {
  const machine = DiscordOwner.useMachine()
  const send = useAtomSet(machine.send)
  const { scope, targetId, mode } = props.override
  const { label, guildLabel, accessibleTarget, confirmDetail } = describeReplyOverride(
    props.override,
    props.directory,
  )
  return (
    <li className="flex min-h-12 flex-wrap items-center gap-x-3 gap-y-2 py-2">
      <span
        aria-hidden="true"
        className="grid size-7 shrink-0 place-items-center rounded-full bg-muted text-[11px] font-semibold text-muted-foreground"
      >
        {scope === 'server' ? <Server className="size-3.5" /> : <Hash className="size-3.5" />}
      </span>
      <div className="flex min-w-0 flex-1 basis-40 flex-wrap items-baseline gap-x-2 gap-y-0.5">
        <span className="break-words text-sm [overflow-wrap:anywhere]">{label}</span>
        {guildLabel === null ? null : (
          <span className="break-words text-xs text-muted-foreground [overflow-wrap:anywhere]">
            {guildLabel}
          </span>
        )}
      </div>
      <div className="ml-auto flex items-center gap-1">
        <ToggleGroup
          aria-label={`Reply mode for ${accessibleTarget}`}
          spacing={1}
          disabled={props.disabled}
          value={[mode]}
          onValueChange={values => {
            const next = values[0]
            if (isReplyMode(next) && next !== mode) {
              send(DiscordEvents.SetReplyMode({ scope, targetId, mode: next }))
            }
          }}
          className="h-10 rounded-full bg-muted p-[3px] md:h-8"
        >
          {replyModes.map(entry => (
            <ToggleGroupItem
              key={entry.mode}
              value={entry.mode}
              className={modeToggleItemClassName}
            >
              {entry.label}
            </ToggleGroupItem>
          ))}
        </ToggleGroup>
        <ConfirmDialog
          tooltip="Remove override"
          trigger={
            <Button
              variant="ghost"
              size="icon"
              aria-label={`Remove reply mode override for ${accessibleTarget}`}
              disabled={props.disabled}
              className="text-destructive hover:bg-destructive/10 hover:text-destructive"
            >
              <Trash2 />
            </Button>
          }
          title="Remove reply mode override?"
          detail={confirmDetail}
          confirmLabel="Remove"
          onConfirm={() => send(DiscordEvents.ClearReplyMode({ scope, targetId }))}
        />
      </div>
    </li>
  )
}

interface ReplyAddDialogProps {
  readonly open: boolean
  readonly isIdle: boolean
  readonly error: string | null
}

/** Dialog to add a channel or whole-server reply mode override (`auto`, `thread`, or `direct`). */
function ReplyAddDialog(props: ReplyAddDialogProps) {
  const machine = DiscordOwner.useMachine()
  const send = useAtomSet(machine.send)
  return (
    <Dialog
      open={props.open}
      onOpenChange={open => {
        if (!open && props.isIdle) send(DiscordEvents.CloseReplyDialog())
      }}
    >
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Add reply mode override</DialogTitle>
        </DialogHeader>
        <MachineState machine={machine} path="">
          {({ value }) => {
            const scope = value.replyScope
            const overridden = (targetId: string) =>
              value.discord.replyModes.some(
                entry => entry.scope === scope && entry.targetId === targetId,
              )
            const isKnownThread = (targetId: string) =>
              value.directory.channels.some(
                channel => channel.id === targetId && channel.kind === 'thread',
              )
            const options =
              scope === 'server'
                ? value.directory.guilds
                    .filter(guild => !overridden(guild.id))
                    .map(guild => ({ value: guild.id, label: guild.name, hint: guild.id }))
                : value.directory.channels
                    .filter(channel => channel.kind === 'channel' && !overridden(channel.id))
                    .toSorted(
                      (a, b) => a.server.localeCompare(b.server) || a.name.localeCompare(b.name),
                    )
                    .map(channel => ({
                      value: channel.id,
                      label: `#${channel.name}`,
                      hint: null,
                      group: channel.server,
                    }))
            const targetValid =
              SNOWFLAKE.test(value.replyTarget) &&
              !overridden(value.replyTarget) &&
              (scope === 'server' || !isKnownThread(value.replyTarget))
            return (
              <div className="space-y-1">
                <FieldRow labelId="discord-reply-scope-label" label="For">
                  <ToggleGroup
                    aria-labelledby="discord-reply-scope-label"
                    spacing={1}
                    disabled={!props.isIdle}
                    value={[scope]}
                    onValueChange={values => {
                      const next = values[0]
                      if (isReplyScope(next) && next !== scope) {
                        send(DiscordEvents.SelectReplyScope({ scope: next }))
                      }
                    }}
                    className="h-10 rounded-full bg-muted p-[3px] md:h-8"
                  >
                    {replyScopes.map(item => (
                      <ToggleGroupItem
                        key={item.scope}
                        value={item.scope}
                        className={modeToggleItemClassName}
                      >
                        {item.label}
                      </ToggleGroupItem>
                    ))}
                  </ToggleGroup>
                </FieldRow>
                <FieldRow id="discord-reply-target" label={replyTargetField[scope].label}>
                  <Picker
                    id="discord-reply-target"
                    placeholder={replyTargetField[scope].placeholder}
                    value={value.replyTarget}
                    disabled={!props.isIdle}
                    clearOnEdit
                    options={options}
                    customOption={raw => {
                      const id = raw.trim()
                      if (
                        !SNOWFLAKE.test(id) ||
                        overridden(id) ||
                        (scope === 'channel' && isKnownThread(id))
                      ) {
                        return null
                      }
                      return { value: id, label: `Use ID ${id}`, hint: null }
                    }}
                    emptyMessage={raw => {
                      const trimmed = raw.trim()
                      if (trimmed === '') return 'No matches'
                      if (overridden(trimmed)) return 'Already has an override'
                      if (scope === 'channel' && isKnownThread(trimmed)) {
                        return 'Reply mode applies to channels, not threads'
                      }
                      if (/^\d+$/.test(trimmed) && !SNOWFLAKE.test(trimmed)) {
                        return 'Discord IDs have 17–20 digits'
                      }
                      return 'No matches'
                    }}
                    onChange={targetId => send(DiscordEvents.SelectReplyTarget({ targetId }))}
                  />
                </FieldRow>
                <FieldRow
                  labelId="discord-reply-setting-label"
                  label="Mode"
                  labelExtra={<ModeHelp label="About reply modes" items={replyModeHelp} />}
                >
                  <ToggleGroup
                    aria-labelledby="discord-reply-setting-label"
                    spacing={1}
                    disabled={!props.isIdle}
                    value={[value.replySetting]}
                    onValueChange={values => {
                      const next = values[0]
                      if (isReplyMode(next) && next !== value.replySetting) {
                        send(DiscordEvents.SelectReplySetting({ mode: next }))
                      }
                    }}
                    className="h-10 rounded-full bg-muted p-[3px] md:h-8"
                  >
                    {replyModes.map(entry => (
                      <ToggleGroupItem
                        key={entry.mode}
                        value={entry.mode}
                        className={modeToggleItemClassName}
                      >
                        {entry.label}
                      </ToggleGroupItem>
                    ))}
                  </ToggleGroup>
                </FieldRow>
                {props.error === null ? null : (
                  <FieldError id="discord-reply-dialog-error">{props.error}</FieldError>
                )}
                <DialogFooter className="mt-4">
                  <Button
                    type="button"
                    variant="ghost"
                    disabled={!props.isIdle}
                    onClick={() => send(DiscordEvents.CloseReplyDialog())}
                  >
                    Cancel
                  </Button>
                  <Button
                    type="button"
                    disabled={!props.isIdle || !targetValid}
                    onClick={() =>
                      send(
                        DiscordEvents.SetReplyMode({
                          scope,
                          targetId: value.replyTarget,
                          mode: value.replySetting,
                        }),
                      )
                    }
                  >
                    {props.isIdle ? 'Add' : 'Saving…'}
                  </Button>
                </DialogFooter>
              </div>
            )
          }}
        </MachineState>
      </DialogContent>
    </Dialog>
  )
}

type HomeKind = Home['kind']

const homeKinds: ReadonlyArray<{ readonly kind: HomeKind; readonly label: string }> = [
  { kind: 'none', label: 'None' },
  { kind: 'channel', label: 'Channel' },
  { kind: 'dm', label: 'Direct message' },
]

function HomeSection() {
  const machine = DiscordOwner.useMachine()
  const isSaving = AsyncResult.getOrElse(
    useMachineResult(AtomMachine.matches(machine, 'HomeChannel.Saving')),
    () => false,
  )
  return (
    <Section title="Home channel">
      <MachineState machine={machine} path="">
        {({ value }) => (
          <HomeForm
            home={value.discord.home}
            pendingKind={value.pendingHomeKind}
            directory={value.directory}
            isSaving={isSaving}
          />
        )}
      </MachineState>
      <HomeFeedback />
    </Section>
  )
}

interface HomeFormProps {
  readonly home: Home
  readonly pendingKind: HomeKind | null
  readonly directory: DiscordDirectory
  readonly isSaving: boolean
}

function HomeForm(props: HomeFormProps) {
  const machine = DiscordOwner.useMachine()
  const send = useAtomSet(machine.send)
  const kind = props.pendingKind ?? props.home.kind
  return (
    <>
      <FieldRow labelId="discord-home-kind-label" label="Type">
        <ToggleGroup
          aria-labelledby="discord-home-kind-label"
          spacing={1}
          disabled={props.isSaving}
          value={[kind]}
          onValueChange={values => {
            const next = homeKinds.find(option => option.kind === values[0])?.kind
            if (next === undefined) return
            if (next === 'none') {
              send(DiscordEvents.SelectHomeKind({ kind: 'none' }))
              if (props.home.kind !== 'none') {
                send(DiscordEvents.SetHome({ home: { kind: 'none' } }))
              }
            } else {
              send(DiscordEvents.SelectHomeKind({ kind: next }))
            }
          }}
          className="h-10 rounded-full bg-muted p-[3px] md:h-8"
        >
          {homeKinds.map(option => (
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
      {kind === 'channel' ? (
        <FieldRow id="discord-home-channel" label="Destination">
          <div className="max-w-80">
            <Picker
              id="discord-home-channel"
              placeholder="Choose a channel"
              focusOnMount={props.pendingKind === 'channel'}
              disabled={props.isSaving}
              value={props.home.kind === 'channel' ? props.home.id : ''}
              options={props.directory.channels
                .toSorted(
                  (a, b) => a.server.localeCompare(b.server) || a.name.localeCompare(b.name),
                )
                .map(channel => ({
                  value: channel.id,
                  label: `#${channel.name}`,
                  hint: null,
                  group: channel.server,
                }))}
              onChange={id => send(DiscordEvents.SetHome({ home: { kind: 'channel', id } }))}
            />
          </div>
        </FieldRow>
      ) : null}
      {kind === 'dm' ? (
        <FieldRow id="discord-home-person" label="Destination">
          <div className="max-w-80">
            <Picker
              id="discord-home-person"
              placeholder="Choose a person"
              focusOnMount={props.pendingKind === 'dm'}
              disabled={props.isSaving}
              value={props.home.kind === 'dm' ? props.home.user : ''}
              options={props.directory.people.map(person => ({
                value: person.id,
                label: person.name,
                hint: person.id,
              }))}
              onChange={user => send(DiscordEvents.SetHome({ home: { kind: 'dm', user } }))}
            />
          </div>
        </FieldRow>
      ) : null}
    </>
  )
}

function HomeFeedback() {
  const machine = DiscordOwner.useMachine()
  const send = useAtomSet(machine.send)
  return (
    <div className="md:grid md:grid-cols-[104px_minmax(0,1fr)] md:gap-4">
      <span className="hidden md:block" />
      <div className="mt-1.5 flex flex-wrap items-center gap-3 text-xs text-muted-foreground">
        <MachineState machine={machine} path="HomeChannel.Idle">
          {({ value }) => {
            const failedHome = value.failedHome
            return value.error !== null ? (
              <>
                <span
                  role="alert"
                  className="break-words text-destructive [overflow-wrap:anywhere]"
                >
                  {value.error}
                </span>
                {failedHome === null ? null : (
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    onClick={() => send(DiscordEvents.SetHome({ home: failedHome }))}
                  >
                    Retry
                  </Button>
                )}
              </>
            ) : null
          }}
        </MachineState>
        <span aria-live="polite" className="empty:sr-only">
          <MachineState machine={machine} path="">
            {({ value, states }) => {
              const homeState = states.HomeChannel.state
              if (homeState.path === 'HomeChannel.Saving') return 'Saving…'
              if (homeState.value.error !== null) return null
              if (
                value.pendingHomeKind !== null &&
                value.pendingHomeKind !== value.discord.home.kind
              ) {
                return value.pendingHomeKind === 'channel'
                  ? 'Choose a channel to save'
                  : 'Choose a person to save'
              }
              return homeState.value.saved ? 'Saved' : null
            }}
          </MachineState>
        </span>
      </div>
    </div>
  )
}
