import type { Secret, SecretsPayload } from '@blob-1/contracts'
import { useAtomSet } from '@effect/atom-react'
import { AtomMachine } from '@typeonce/effect-machine/reactivity'
import { createMachineContext, useMachineResult } from '@typeonce/effect-machine-react'
import { Option } from 'effect'
import { AsyncResult } from 'effect/reactivity'
import { Check } from 'lucide-react'
import { useEffect, useState } from 'react'

import { EmptyLine, FieldError, FieldRow } from '~/components/layout/Page'
import { Button } from '~/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '~/components/ui/dialog'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '~/components/ui/select'
import { MachineAtoms } from '~/lib/runtime'
import { cn } from '~/lib/utils'

import {
  ImportSecretsEvents,
  ImportSecretsMachine,
  type ImportSecretsInput,
  type ImportSecretsSource,
} from './importSecretsMachine'
import { formatSecretUpdatedAt } from './secretsState'

const ImportSecretsOwner = createMachineContext(
  ({ input }: { readonly input: ImportSecretsInput }) =>
    MachineAtoms.make(ImportSecretsMachine, input),
)

interface ImportCompletionProps {
  readonly state: SecretsPayload
  readonly onImported: (state: SecretsPayload) => void
}

function ImportCompletion(props: ImportCompletionProps) {
  const machine = ImportSecretsOwner.useMachine()
  const send = useAtomSet(machine.send)
  const { onImported, state } = props

  useEffect(() => {
    onImported(state)
    send(ImportSecretsEvents.Close())
  }, [onImported, send, state])

  return null
}

interface SecretChecklistProps {
  readonly sourceName: string
  readonly secrets: ReadonlyArray<Secret>
  readonly currentSecrets: ReadonlyArray<Secret>
  readonly error: string | null
  readonly importing: boolean
}

function SecretChecklist(props: SecretChecklistProps) {
  const machine = ImportSecretsOwner.useMachine()
  const send = useAtomSet(machine.send)
  const selectableNames = props.secrets.map(secret => secret.name)
  const [checked, setChecked] = useState<ReadonlySet<string>>(() => new Set(selectableNames))

  const selectedNames = props.secrets
    .filter(secret => checked.has(secret.name))
    .map(secret => secret.name)
  const allSelected = selectableNames.length > 0 && selectableNames.every(name => checked.has(name))

  const clearError = () => {
    if (props.error !== null) send(ImportSecretsEvents.ClearError())
  }

  const toggleSecret = (secret: Secret) => {
    setChecked(current => {
      const next = new Set(current)
      if (next.has(secret.name)) {
        next.delete(secret.name)
      } else {
        next.add(secret.name)
      }
      return next
    })
    clearError()
  }

  const toggleAll = () => {
    setChecked(allSelected ? new Set() : new Set(selectableNames))
    clearError()
  }

  return (
    <div className="space-y-3">
      <div className="flex items-center justify-between gap-3">
        <p className="min-w-0 truncate text-sm font-medium">{props.sourceName}</p>
        <Button
          type="button"
          variant="ghost"
          size="xs"
          disabled={props.importing || selectableNames.length === 0}
          onClick={toggleAll}
        >
          {allSelected ? 'Select none' : 'Select all'}
        </Button>
      </div>
      {props.secrets.length === 0 ? (
        <EmptyLine>No secrets found.</EmptyLine>
      ) : (
        <ul className="-mx-4 divide-y divide-border/60 border-t border-border/60">
          {props.secrets.map(secret => {
            const disabled = props.importing
            const isChecked = checked.has(secret.name)
            const replaces = props.currentSecrets.some(entry => entry.name === secret.name)
            return (
              <li key={secret.name}>
                <label
                  className={cn(
                    'flex w-full items-center gap-3 px-4 py-2.5 text-left transition-colors',
                    disabled
                      ? 'cursor-not-allowed opacity-60'
                      : 'cursor-pointer hover:bg-accent/60',
                  )}
                >
                  <input
                    type="checkbox"
                    checked={isChecked}
                    disabled={disabled}
                    onChange={() => toggleSecret(secret)}
                    className="peer sr-only"
                  />
                  <div className="min-w-0 flex-1 rounded-lg peer-focus-visible:ring-2 peer-focus-visible:ring-inset peer-focus-visible:ring-ring/50">
                    <div className="flex min-w-0 items-center gap-2">
                      <span className="min-w-0 truncate font-mono text-sm font-medium">
                        {secret.name}
                      </span>
                      {replaces ? (
                        <span className="ml-auto shrink-0 text-xs text-muted-foreground">
                          Replaces
                        </span>
                      ) : null}
                    </div>
                    <time
                      className="mt-0.5 block truncate text-xs text-muted-foreground tabular-nums"
                      dateTime={new Date(secret.updatedAt * 1000).toISOString()}
                    >
                      Updated {formatSecretUpdatedAt(secret.updatedAt)}
                    </time>
                  </div>
                  <span
                    className={cn(
                      'grid size-5 shrink-0 place-items-center rounded-full border transition-colors',
                      isChecked
                        ? 'border-foreground bg-foreground text-background'
                        : 'border-border text-transparent',
                    )}
                  >
                    <Check className="size-3" />
                  </span>
                </label>
              </li>
            )
          })}
        </ul>
      )}
      {props.error === null ? null : <FieldError>{props.error}</FieldError>}
      <DialogFooter className="sticky -bottom-4 z-10 bg-popover">
        <Button
          type="button"
          variant="ghost"
          disabled={props.importing}
          onClick={() => send(ImportSecretsEvents.Close())}
        >
          Cancel
        </Button>
        <Button
          type="button"
          disabled={props.importing || selectedNames.length === 0}
          onClick={() => send(ImportSecretsEvents.Import({ names: selectedNames }))}
        >
          {props.importing ? 'Importing…' : `Import ${selectedNames.length}`}
        </Button>
      </DialogFooter>
    </div>
  )
}

interface ImportSecretsDialogBodyProps {
  readonly sourceBots: ReadonlyArray<ImportSecretsSource>
  readonly currentSecrets: ReadonlyArray<Secret>
  readonly onImported: (state: SecretsPayload) => void
}

function useImportSecretsState() {
  const machine = ImportSecretsOwner.useMachine()

  const picking = Option.getOrNull(
    AsyncResult.getOrElse(useMachineResult(AtomMachine.select(machine, 'Picking')), () =>
      Option.none(),
    ),
  )
  const loading = Option.getOrNull(
    AsyncResult.getOrElse(useMachineResult(AtomMachine.select(machine, 'Loading')), () =>
      Option.none(),
    ),
  )
  const choosing = Option.getOrNull(
    AsyncResult.getOrElse(useMachineResult(AtomMachine.select(machine, 'Choosing')), () =>
      Option.none(),
    ),
  )
  const importing = Option.getOrNull(
    AsyncResult.getOrElse(useMachineResult(AtomMachine.select(machine, 'Importing')), () =>
      Option.none(),
    ),
  )
  const imported = Option.getOrNull(
    AsyncResult.getOrElse(useMachineResult(AtomMachine.select(machine, 'Imported')), () =>
      Option.none(),
    ),
  )

  const choiceState = choosing ?? importing
  const selectedSource = loading?.source ?? choiceState?.source ?? null
  const error = picking?.error ?? choosing?.error ?? null

  return { machine, loading, choosing, importing, imported, choiceState, selectedSource, error }
}

function ImportSecretsDialogBody(props: ImportSecretsDialogBodyProps) {
  const { machine, loading, choosing, importing, imported, choiceState, selectedSource, error } =
    useImportSecretsState()
  const send = useAtomSet(machine.send)
  const sourceItems = props.sourceBots.map(bot => ({ value: bot.id, label: bot.name }))

  return (
    <div className="space-y-2">
      <FieldRow id="import-secrets-source" label="Source bot">
        <Select
          value={selectedSource?.id ?? ''}
          items={sourceItems}
          disabled={loading !== null || importing !== null}
          onValueChange={id => {
            if (id === null) return
            const source = props.sourceBots.find(bot => bot.id === id)
            if (source !== undefined) {
              send(
                ImportSecretsEvents.SelectSource({
                  id: source.id,
                  name: source.name,
                  generation: source.generation,
                }),
              )
            }
          }}
        >
          <SelectTrigger id="import-secrets-source" className="w-full">
            <SelectValue placeholder="Choose a bot" />
          </SelectTrigger>
          <SelectContent>
            {sourceItems.map(item => (
              <SelectItem key={item.value} value={item.value}>
                {item.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </FieldRow>
      {choiceState === null && error !== null ? <FieldError>{error}</FieldError> : null}
      {loading === null ? null : <EmptyLine>Loading…</EmptyLine>}
      {choiceState === null ? null : (
        <SecretChecklist
          key={`${choiceState.source.id}@${choiceState.source.generation}`}
          sourceName={choiceState.source.name}
          secrets={choiceState.secrets.secrets}
          currentSecrets={props.currentSecrets}
          error={choosing?.error ?? null}
          importing={importing !== null}
        />
      )}
      {imported === null ? null : (
        <ImportCompletion state={imported.secrets} onImported={props.onImported} />
      )}
    </div>
  )
}

interface ImportSecretsControlProps {
  readonly scopeKey: string
  readonly targetBotId: string
  readonly targetBotGeneration: number | null
  readonly sourceBots: ReadonlyArray<ImportSecretsSource>
  readonly currentSecrets: ReadonlyArray<Secret>
  readonly onImported: (state: SecretsPayload) => void
}

function ImportSecretsTriggerAndDialog(props: ImportSecretsControlProps) {
  const machine = ImportSecretsOwner.useMachine()
  const send = useAtomSet(machine.send)
  const closed = AsyncResult.getOrElse(
    useMachineResult(AtomMachine.matches(machine, 'Closed')),
    () => true,
  )
  const open = !closed

  return (
    <>
      {props.sourceBots.length === 0 ? null : (
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={() => send(ImportSecretsEvents.Open())}
        >
          Import from bot
        </Button>
      )}
      <Dialog
        open={open}
        onOpenChange={next => send(next ? ImportSecretsEvents.Open() : ImportSecretsEvents.Close())}
      >
        <DialogContent className="max-h-[calc(100dvh-4rem)] grid-cols-[minmax(0,1fr)] overflow-y-auto sm:max-w-xl">
          <DialogHeader className="sticky -top-4 z-10 -mx-4 -mt-4 bg-popover px-4 pt-4 pb-1">
            <DialogTitle>Import from bot</DialogTitle>
          </DialogHeader>
          {open ? (
            <ImportSecretsDialogBody
              sourceBots={props.sourceBots}
              currentSecrets={props.currentSecrets}
              onImported={props.onImported}
            />
          ) : null}
        </DialogContent>
      </Dialog>
    </>
  )
}

/** Dialog for copying selected secrets from another bot into this bot. */
export function ImportSecretsControl(props: ImportSecretsControlProps) {
  return (
    <ImportSecretsOwner.Provider
      key={props.scopeKey}
      input={{
        targetBotId: props.targetBotId,
        targetBotGeneration: props.targetBotGeneration,
      }}
    >
      <ImportSecretsTriggerAndDialog {...props} />
    </ImportSecretsOwner.Provider>
  )
}
