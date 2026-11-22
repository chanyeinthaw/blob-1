import type { ApiError } from '@blob-1/client/api'
import type {
  DiscordConnection,
  DiscordDirectory,
  DiscordInteractionOverride,
  DiscordReplyMode,
  DiscordReplyOverride,
  Home,
} from '@blob-1/contracts'
import { useAtomRefresh, useAtomSet, useAtomValue } from '@effect/atom-react'
import { AtomMachine } from '@typeonce/effect-machine/reactivity'
import {
  createMachineContext,
  MachineState,
  useMachineResult,
} from '@typeonce/effect-machine-react'
import { type Cause, Option } from 'effect'
import { AsyncResult } from 'effect/reactivity'
import { Hash, KeyRound, Pencil, Trash2, User } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'

import { ConfirmDialog } from '~/components/ConfirmDialog'
import { EmptyLine, FieldError, FieldRow, Section } from '~/components/layout/Page'
import { Picker } from '~/components/Picker'
import { Button } from '~/components/ui/button'
import { Input } from '~/components/ui/input'
import { ToggleGroup, ToggleGroupItem } from '~/components/ui/toggle-group'
import { Tooltip, TooltipContent, TooltipTrigger } from '~/components/ui/tooltip'
import { MachineAtoms } from '~/lib/runtime'
import { cn } from '~/lib/utils'

import { DiscordEvents, type DiscordInput, DiscordMachine } from './discordMachine'
import { discordPageAtom } from './discordState'

const DiscordOwner = createMachineContext(({ input }: { input: DiscordInput }) =>
  MachineAtoms.make(DiscordMachine, input),
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
  return AsyncResult.match(result, {
    onInitial: () => props.children({ kind: 'loading' }),
    onFailure: failure => props.children({ kind: 'failed', cause: failure.cause, onRetry: retry }),
    onSuccess: ({ value }) => (
      <DiscordOwner.Provider input={value}>
        {props.children({ kind: 'loaded' })}
      </DiscordOwner.Provider>
    ),
  })
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
      <AllowlistSection />
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

function AllowlistSection() {
  const machine = DiscordOwner.useMachine()
  const isIdle = AsyncResult.getOrElse(
    useMachineResult(AtomMachine.matches(machine, 'Allowlist.Idle')),
    () => false,
  )
  return (
    <Section title="Who can talk to Blob-1">
      <MachineState machine={machine} path="">
        {({ value }) =>
          value.discord.allowed.length === 0 ? (
            <EmptyLine>Nobody can talk to Blob-1 yet.</EmptyLine>
          ) : (
            <ul className="flex flex-col divide-y">
              {value.discord.allowed.map(user => (
                <AllowedRow
                  key={user}
                  user={user}
                  name={personName(value.directory, user)}
                  isIdle={isIdle}
                />
              ))}
            </ul>
          )
        }
      </MachineState>
      <AllowForm />
    </Section>
  )
}

interface AllowedRowProps {
  readonly user: string
  readonly name: string | undefined
  readonly isIdle: boolean
}

function AllowedRow(props: AllowedRowProps) {
  const machine = DiscordOwner.useMachine()
  const send = useAtomSet(machine.send)
  return (
    <li className="flex min-h-12 items-center gap-3 py-2">
      <span
        aria-hidden="true"
        className="grid size-7 shrink-0 place-items-center rounded-full bg-muted text-[11px] font-semibold text-muted-foreground"
      >
        {props.name === undefined ? <User className="size-3.5" /> : initials(props.name)}
      </span>
      <div className="flex min-w-0 flex-1 flex-wrap items-baseline gap-x-2 gap-y-0.5">
        {props.name === undefined ? (
          <span className="break-all font-mono text-sm">{props.user}</span>
        ) : (
          <>
            <span className="break-words text-sm [overflow-wrap:anywhere]">{props.name}</span>
            <span className="font-mono text-xs text-muted-foreground">{props.user}</span>
          </>
        )}
      </div>
      <ConfirmDialog
        tooltip="Remove from allowlist"
        trigger={
          <Button
            variant="ghost"
            size="icon"
            aria-label={`Remove ${props.name ?? props.user}`}
            disabled={!props.isIdle}
            className="text-destructive hover:bg-destructive/10 hover:text-destructive"
          >
            <Trash2 />
          </Button>
        }
        title="Remove from allowlist?"
        detail={props.name === undefined ? props.user : `${props.name} (${props.user})`}
        confirmLabel="Remove"
        onConfirm={() => send(DiscordEvents.Disallow({ user: props.user }))}
      />
    </li>
  )
}

function AllowForm() {
  const machine = DiscordOwner.useMachine()
  const send = useAtomSet(machine.send)
  const [localError, setLocalError] = useState('')
  const idle = Option.getOrNull(
    AsyncResult.getOrElse(useMachineResult(AtomMachine.select(machine, 'Allowlist.Idle')), () =>
      Option.none(),
    ),
  )
  const adding = Option.getOrNull(
    AsyncResult.getOrElse(useMachineResult(AtomMachine.select(machine, 'Allowlist.Adding')), () =>
      Option.none(),
    ),
  )
  const draft = idle?.draft ?? adding?.user ?? ''
  const error = localError === '' ? (idle?.error ?? null) : localError
  return (
    <MachineState machine={machine} path="">
      {({ value }) => {
        const availablePeople = value.directory.people.filter(
          person => !value.discord.allowed.includes(person.id),
        )
        return availablePeople.length > 0 ? (
          <div className="mt-2.5">
            <label htmlFor="discord-allow-picker" className="sr-only">
              Add a person or Discord user ID
            </label>
            <Picker
              id="discord-allow-picker"
              placeholder="Add a person or paste a user ID"
              value=""
              clearOnSelect
              disabled={idle === null}
              options={availablePeople.map(person => ({
                value: person.id,
                label: person.name,
                hint: person.id,
              }))}
              customOption={raw => {
                const user = raw.trim()
                if (!SNOWFLAKE.test(user) || value.discord.allowed.includes(user)) return null
                return { value: user, label: `Allow ID ${user}`, hint: null }
              }}
              emptyMessage={raw => {
                const trimmed = raw.trim()
                if (trimmed === '') return 'No matches'
                if (value.discord.allowed.includes(trimmed)) return 'Already allowed'
                if (/^\d+$/.test(trimmed) && !SNOWFLAKE.test(trimmed)) {
                  return 'Discord user IDs have 17–20 digits'
                }
                return 'No matches'
              }}
              onChange={user => {
                setLocalError('')
                send(DiscordEvents.Allow({ user }))
              }}
            />
            {adding === null ? null : (
              <p aria-live="polite" className="mt-1.5 px-3.5 text-xs text-muted-foreground">
                Adding…
              </p>
            )}
            {error === null ? null : <FieldError id="discord-allow-error">{error}</FieldError>}
          </div>
        ) : (
          <form
            noValidate
            className="mt-2.5"
            onSubmit={event => {
              event.preventDefault()
              const user = draft.trim()
              if (!SNOWFLAKE.test(user)) {
                setLocalError('Enter a Discord user ID with 17–20 digits.')
              } else if (value.discord.allowed.includes(user)) {
                setLocalError('This user is already allowed.')
              } else {
                setLocalError('')
                send(DiscordEvents.Allow({ user }))
              }
            }}
          >
            <label htmlFor="discord-allow" className="sr-only">
              Discord user ID
            </label>
            <div className="flex items-start gap-2">
              <div className="min-w-0 flex-1">
                <Input
                  id="discord-allow"
                  inputMode="numeric"
                  placeholder="Discord user ID"
                  className="font-mono"
                  value={draft}
                  readOnly={idle === null}
                  aria-invalid={error !== null}
                  aria-describedby={error === null ? undefined : 'discord-allow-error'}
                  onChange={event => {
                    setLocalError('')
                    send(DiscordEvents.EditAllowDraft({ draft: event.target.value }))
                  }}
                />
                {error === null ? null : <FieldError id="discord-allow-error">{error}</FieldError>}
              </div>
              <Button
                type="submit"
                variant="outline"
                disabled={idle === null || draft.trim() === ''}
              >
                {adding !== null ? 'Adding…' : 'Allow'}
              </Button>
            </div>
          </form>
        )
      }}
    </MachineState>
  )
}

const modeToggleItemClassName =
  'h-full rounded-full px-3 text-muted-foreground hover:bg-transparent hover:text-foreground aria-pressed:bg-background aria-pressed:text-foreground aria-pressed:shadow-sm'

/** Per-channel and per-person overrides of the default mention-only gate. */
function InteractionModeSection() {
  const machine = DiscordOwner.useMachine()
  const isIdle = AsyncResult.getOrElse(
    useMachineResult(AtomMachine.matches(machine, 'Interaction.Idle')),
    () => false,
  )
  return (
    <Section title="Interaction mode">
      <MachineState machine={machine} path="">
        {({ value }) =>
          value.discord.interactionModes.length === 0 ? (
            <EmptyLine>No interaction mode overrides.</EmptyLine>
          ) : (
            <ul className="flex flex-col divide-y">
              {value.discord.interactionModes.map(override => (
                <OverrideRow
                  key={`${override.scope}:${override.guildId ?? ''}:${override.targetId}`}
                  override={override}
                  directory={value.directory}
                  disabled={!isIdle}
                />
              ))}
            </ul>
          )
        }
      </MachineState>
      <ModeForm isIdle={isIdle} />
    </Section>
  )
}

interface OverrideRowProps {
  readonly override: DiscordInteractionOverride
  readonly directory: DiscordDirectory
  readonly disabled: boolean
}

function resolveOverrideTarget(override: DiscordInteractionOverride, directory: DiscordDirectory) {
  if (override.scope === 'channel') {
    const channel = directory.channels.find(entry => entry.id === override.targetId)
    const name = channel?.name
    return {
      name,
      label: name === undefined ? override.targetId : `#${name}`,
      guildLabel: channel?.server ?? null,
    }
  }
  const name = directory.people.find(person => person.id === override.targetId)?.name
  const guildLabel =
    override.guildId === null
      ? 'Unassigned server (inactive)'
      : (directory.guilds.find(guild => guild.id === override.guildId)?.name ??
        `server ${override.guildId}`)
  return { name, label: name ?? override.targetId, guildLabel }
}

function describeOverride(override: DiscordInteractionOverride, directory: DiscordDirectory) {
  const { name, label, guildLabel } = resolveOverrideTarget(override, directory)
  const detail = [label, guildLabel].filter(Boolean).join(' · ')
  return {
    name,
    label,
    guildLabel,
    accessibleTarget: guildLabel === null ? label : `${label} in ${guildLabel}`,
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
        {name === undefined ? null : (
          <span className="font-mono text-xs text-muted-foreground">{targetId}</span>
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
            if ((next === 'mention-only' || next === 'always') && next !== mode) {
              send(DiscordEvents.SetInteractionMode({ scope, guildId, targetId, mode: next }))
            }
          }}
          className="h-10 rounded-full bg-muted p-[3px] md:h-8"
        >
          <ToggleGroupItem value="mention-only" className={modeToggleItemClassName}>
            Mention only
          </ToggleGroupItem>
          <ToggleGroupItem value="always" className={modeToggleItemClassName}>
            Always
          </ToggleGroupItem>
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

interface ModeFormProps {
  readonly isIdle: boolean
}

/** The add-override form: scope toggle, optional server picker, target picker and mode toggle. */
function ModeForm(props: ModeFormProps) {
  const machine = DiscordOwner.useMachine()
  const send = useAtomSet(machine.send)
  return (
    <MachineState machine={machine} path="">
      {({ value, states }) => {
        const interaction = states.Interaction.state
        const error = interaction.path === 'Interaction.Idle' ? interaction.value.error : null
        const scope = value.modeScope
        const selectedGuild = scope === 'user' ? value.modeGuild : null
        const overridden = (targetId: string) =>
          value.discord.interactionModes.some(
            entry =>
              entry.scope === scope &&
              (scope === 'channel' || entry.guildId === selectedGuild) &&
              entry.targetId === targetId,
          )
        const guildOptions = value.directory.guilds.map(guild => ({
          value: guild.id,
          label: guild.name,
          hint: guild.id,
        }))
        const options =
          scope === 'channel'
            ? value.directory.channels
                .filter(channel => !overridden(channel.id))
                .map(channel => ({
                  value: channel.id,
                  label: `#${channel.name}`,
                  hint: channel.server,
                }))
            : value.directory.people
                .filter(
                  person =>
                    value.discord.allowed.includes(person.id) &&
                    !overridden(person.id) &&
                    (selectedGuild === null ||
                      selectedGuild === '' ||
                      person.guilds.includes(selectedGuild)),
                )
                .map(person => ({ value: person.id, label: person.name, hint: person.id }))
        const guildValid = scope === 'channel' || SNOWFLAKE.test(value.modeGuild)
        const targetValid =
          guildValid && SNOWFLAKE.test(value.modeTarget) && !overridden(value.modeTarget)
        return (
          <div className="mt-2.5">
            <FieldRow labelId="discord-mode-scope-label" label="For">
              <ToggleGroup
                aria-labelledby="discord-mode-scope-label"
                spacing={1}
                disabled={!props.isIdle}
                value={[scope]}
                onValueChange={values => {
                  const next = values[0]
                  if ((next === 'channel' || next === 'user') && next !== scope) {
                    send(DiscordEvents.SelectModeScope({ scope: next }))
                  }
                }}
                className="h-10 rounded-full bg-muted p-[3px] md:h-8"
              >
                <ToggleGroupItem value="channel" className={modeToggleItemClassName}>
                  Channel
                </ToggleGroupItem>
                <ToggleGroupItem value="user" className={modeToggleItemClassName}>
                  Person
                </ToggleGroupItem>
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
            <FieldRow id="discord-mode-target" label={scope === 'channel' ? 'Channel' : 'Person'}>
              <Picker
                id="discord-mode-target"
                placeholder={scope === 'channel' ? 'Choose a channel' : 'Choose a person'}
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
            <FieldRow labelId="discord-mode-setting-label" label="Mode">
              <div className="flex flex-wrap items-center gap-2">
                <ToggleGroup
                  aria-labelledby="discord-mode-setting-label"
                  spacing={1}
                  disabled={!props.isIdle}
                  value={[value.modeSetting]}
                  onValueChange={values => {
                    const next = values[0]
                    if (
                      (next === 'mention-only' || next === 'always') &&
                      next !== value.modeSetting
                    ) {
                      send(DiscordEvents.SelectModeSetting({ mode: next }))
                    }
                  }}
                  className="h-10 rounded-full bg-muted p-[3px] md:h-8"
                >
                  <ToggleGroupItem value="mention-only" className={modeToggleItemClassName}>
                    Mention only
                  </ToggleGroupItem>
                  <ToggleGroupItem value="always" className={modeToggleItemClassName}>
                    Always
                  </ToggleGroupItem>
                </ToggleGroup>
                <Button
                  className="ml-auto"
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
                  Add override
                </Button>
              </div>
            </FieldRow>
            {error === null ? null : <FieldError id="discord-mode-error">{error}</FieldError>}
          </div>
        )
      }}
    </MachineState>
  )
}

const replyModes: ReadonlyArray<{ readonly mode: DiscordReplyMode; readonly label: string }> = [
  { mode: 'decision', label: 'Decision' },
  { mode: 'thread', label: 'Thread' },
  { mode: 'direct', label: 'Direct' },
]

function isReplyMode(value: string | undefined): value is DiscordReplyMode {
  return value === 'decision' || value === 'thread' || value === 'direct'
}

function ReplyModeSection() {
  const machine = DiscordOwner.useMachine()
  const isIdle = AsyncResult.getOrElse(
    useMachineResult(AtomMachine.matches(machine, 'ReplyMode.Idle')),
    () => false,
  )
  return (
    <Section title="Reply mode">
      <MachineState machine={machine} path="">
        {({ value }) =>
          value.discord.replyModes.length === 0 ? (
            <EmptyLine>No reply mode overrides.</EmptyLine>
          ) : (
            <ul className="flex flex-col divide-y">
              {value.discord.replyModes.map(override => (
                <ReplyOverrideRow
                  key={override.channelId}
                  override={override}
                  directory={value.directory}
                  disabled={!isIdle}
                />
              ))}
            </ul>
          )
        }
      </MachineState>
      <ReplyModeForm isIdle={isIdle} />
    </Section>
  )
}

interface ReplyOverrideRowProps {
  readonly override: DiscordReplyOverride
  readonly directory: DiscordDirectory
  readonly disabled: boolean
}

function describeReplyOverride(override: DiscordReplyOverride, directory: DiscordDirectory) {
  const channel = directory.channels.find(entry => entry.id === override.channelId)
  const name = channel?.name
  const label = name === undefined ? override.channelId : `#${name}`
  const guildLabel = channel?.server ?? null
  const detail = [label, guildLabel].filter(Boolean).join(' · ')
  return {
    name,
    label,
    guildLabel,
    accessibleTarget: guildLabel === null ? label : `${label} in ${guildLabel}`,
    confirmDetail: name === undefined ? detail : `${detail} (${override.channelId})`,
  }
}

function ReplyOverrideRow(props: ReplyOverrideRowProps) {
  const machine = DiscordOwner.useMachine()
  const send = useAtomSet(machine.send)
  const { channelId, mode } = props.override
  const { name, label, guildLabel, accessibleTarget, confirmDetail } = describeReplyOverride(
    props.override,
    props.directory,
  )
  return (
    <li className="flex min-h-12 flex-wrap items-center gap-x-3 gap-y-2 py-2">
      <span
        aria-hidden="true"
        className="grid size-7 shrink-0 place-items-center rounded-full bg-muted text-[11px] font-semibold text-muted-foreground"
      >
        <Hash className="size-3.5" />
      </span>
      <div className="flex min-w-0 flex-1 basis-40 flex-wrap items-baseline gap-x-2 gap-y-0.5">
        <span className="break-words text-sm [overflow-wrap:anywhere]">{label}</span>
        {guildLabel === null ? null : (
          <span className="break-words text-xs text-muted-foreground [overflow-wrap:anywhere]">
            {guildLabel}
          </span>
        )}
        {name === undefined ? null : (
          <span className="font-mono text-xs text-muted-foreground">{channelId}</span>
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
              send(DiscordEvents.SetReplyMode({ channelId, mode: next }))
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
          onConfirm={() => send(DiscordEvents.ClearReplyMode({ channelId }))}
        />
      </div>
    </li>
  )
}

interface ReplyModeFormProps {
  readonly isIdle: boolean
}

/** Add a channel reply mode override (`decision`, `thread`, or `direct`). */
function ReplyModeForm(props: ReplyModeFormProps) {
  const machine = DiscordOwner.useMachine()
  const send = useAtomSet(machine.send)
  return (
    <MachineState machine={machine} path="">
      {({ value, states }) => {
        const reply = states.ReplyMode.state
        const error = reply.path === 'ReplyMode.Idle' ? reply.value.error : null
        const overridden = (channelId: string) =>
          value.discord.replyModes.some(entry => entry.channelId === channelId)
        const isKnownThread = (channelId: string) =>
          value.directory.channels.some(
            channel => channel.id === channelId && channel.kind === 'thread',
          )
        const options = value.directory.channels
          .filter(channel => channel.kind === 'channel' && !overridden(channel.id))
          .map(channel => ({
            value: channel.id,
            label: `#${channel.name}`,
            hint: channel.server,
          }))
        const channelValid =
          SNOWFLAKE.test(value.replyChannel) &&
          !overridden(value.replyChannel) &&
          !isKnownThread(value.replyChannel)
        return (
          <div className="mt-2.5">
            <FieldRow id="discord-reply-channel" label="Channel">
              <Picker
                id="discord-reply-channel"
                placeholder="Choose a channel"
                value={value.replyChannel}
                disabled={!props.isIdle}
                clearOnEdit
                options={options}
                customOption={raw => {
                  const id = raw.trim()
                  if (!SNOWFLAKE.test(id) || overridden(id) || isKnownThread(id)) return null
                  return { value: id, label: `Use ID ${id}`, hint: null }
                }}
                emptyMessage={raw => {
                  const trimmed = raw.trim()
                  if (trimmed === '') return 'No matches'
                  if (overridden(trimmed)) return 'Already has an override'
                  if (isKnownThread(trimmed)) return 'Reply mode applies to channels, not threads'
                  if (/^\d+$/.test(trimmed) && !SNOWFLAKE.test(trimmed)) {
                    return 'Discord IDs have 17–20 digits'
                  }
                  return 'No matches'
                }}
                onChange={channelId => send(DiscordEvents.SelectReplyChannel({ channelId }))}
              />
            </FieldRow>
            <FieldRow labelId="discord-reply-setting-label" label="Mode">
              <div className="flex flex-wrap items-center gap-2">
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
                <Button
                  className="ml-auto"
                  disabled={!props.isIdle || !channelValid}
                  onClick={() =>
                    send(
                      DiscordEvents.SetReplyMode({
                        channelId: value.replyChannel,
                        mode: value.replySetting,
                      }),
                    )
                  }
                >
                  Add override
                </Button>
              </div>
            </FieldRow>
            {error === null ? null : <FieldError id="discord-reply-error">{error}</FieldError>}
          </div>
        )
      }}
    </MachineState>
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
  const multipleServers = new Set(props.directory.channels.map(channel => channel.server)).size > 1
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
          <Picker
            id="discord-home-channel"
            placeholder="Choose a channel"
            focusOnMount={props.pendingKind === 'channel'}
            disabled={props.isSaving}
            value={props.home.kind === 'channel' ? props.home.id : ''}
            options={props.directory.channels.map(channel => ({
              value: channel.id,
              label: `#${channel.name}`,
              hint: multipleServers ? channel.server : null,
            }))}
            onChange={id => send(DiscordEvents.SetHome({ home: { kind: 'channel', id } }))}
          />
        </FieldRow>
      ) : null}
      {kind === 'dm' ? (
        <FieldRow id="discord-home-person" label="Destination">
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
