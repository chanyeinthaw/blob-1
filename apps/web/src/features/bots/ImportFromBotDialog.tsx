import type { BotSkill, BotSkillsState } from '@blob-1/contracts'
import { useAtomSet } from '@effect/atom-react'
import { AtomMachine } from '@typeonce/effect-machine/reactivity'
import { createMachineContext, useMachineResult } from '@typeonce/effect-machine-react'
import { Option } from 'effect'
import { AsyncResult } from 'effect/reactivity'
import { Check } from 'lucide-react'
import { useEffect, useState, type ReactNode } from 'react'

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
  ImportFromBotEvents,
  ImportFromBotMachine,
  type ImportFromBotInput,
  type ImportFromBotSource,
} from './importFromBotMachine'

const ImportFromBotOwner = createMachineContext(
  ({ input }: { readonly input: ImportFromBotInput }) =>
    MachineAtoms.make(ImportFromBotMachine, input),
)

interface ImportCompletionProps {
  readonly state: BotSkillsState
  readonly onImported: (state: BotSkillsState) => void
}

function ImportCompletion(props: ImportCompletionProps) {
  const machine = ImportFromBotOwner.useMachine()
  const send = useAtomSet(machine.send)
  const { onImported, state } = props

  useEffect(() => {
    onImported(state)
    send(ImportFromBotEvents.Close())
  }, [onImported, send, state])

  return null
}

interface SkillChecklistProps {
  readonly sourceName: string
  readonly skills: ReadonlyArray<BotSkill>
  readonly currentSkills: ReadonlyArray<BotSkill>
  readonly error: string | null
  readonly importing: boolean
}

function SkillChecklist(props: SkillChecklistProps) {
  const machine = ImportFromBotOwner.useMachine()
  const send = useAtomSet(machine.send)
  const selectableNames = props.skills.map(skill => skill.name)
  const [checked, setChecked] = useState<ReadonlySet<string>>(() => new Set(selectableNames))

  const selectedNames = props.skills
    .filter(skill => checked.has(skill.name))
    .map(skill => skill.name)
  const allSelected = selectableNames.length > 0 && selectableNames.every(name => checked.has(name))

  const clearError = () => {
    if (props.error !== null) send(ImportFromBotEvents.ClearError())
  }

  const toggleSkill = (skill: BotSkill) => {
    setChecked(current => {
      const next = new Set(current)
      if (next.has(skill.name)) {
        next.delete(skill.name)
      } else {
        next.add(skill.name)
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
      {props.skills.length === 0 ? (
        <EmptyLine>No skills found.</EmptyLine>
      ) : (
        <ul className="-mx-4 divide-y divide-border/60 border-t border-border/60">
          {props.skills.map(skill => {
            const disabled = props.importing
            const isChecked = checked.has(skill.name)
            const conflict = props.currentSkills.find(entry => entry.name === skill.name)?.source
            return (
              <li key={skill.name}>
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
                    onChange={() => toggleSkill(skill)}
                    className="peer sr-only"
                  />
                  <div className="min-w-0 flex-1 rounded-lg peer-focus-visible:ring-2 peer-focus-visible:ring-inset peer-focus-visible:ring-ring/50">
                    <div className="flex min-w-0 items-center gap-2">
                      <span className="min-w-0 truncate font-mono text-sm font-medium">
                        {skill.name}
                      </span>
                      {conflict === undefined ? null : (
                        <span className="ml-auto shrink-0 text-xs text-muted-foreground">
                          {conflict === 'detected' ? 'Hides detected' : 'Replaces managed'}
                        </span>
                      )}
                    </div>
                    {skill.description === '' ? null : (
                      <p className="mt-0.5 truncate text-xs text-muted-foreground">
                        {skill.description}
                      </p>
                    )}
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
          onClick={() => send(ImportFromBotEvents.Close())}
        >
          Cancel
        </Button>
        <Button
          type="button"
          disabled={props.importing || selectedNames.length === 0}
          onClick={() => send(ImportFromBotEvents.Import({ names: selectedNames }))}
        >
          {props.importing ? 'Importing…' : `Import ${selectedNames.length}`}
        </Button>
      </DialogFooter>
    </div>
  )
}

interface ImportFromBotDialogBodyProps {
  readonly sourceBots: ReadonlyArray<ImportFromBotSource>
  readonly currentSkills: ReadonlyArray<BotSkill>
  readonly onImported: (state: BotSkillsState) => void
}

function useImportFromBotState() {
  const machine = ImportFromBotOwner.useMachine()

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

function ImportFromBotDialogBody(props: ImportFromBotDialogBodyProps) {
  const { machine, loading, choosing, importing, imported, choiceState, selectedSource, error } =
    useImportFromBotState()
  const send = useAtomSet(machine.send)
  const sourceItems = props.sourceBots.map(bot => ({ value: bot.id, label: bot.name }))

  return (
    <div className="space-y-2">
      <FieldRow id="import-from-bot-source" label="Source bot">
        <Select
          value={selectedSource?.id ?? ''}
          items={sourceItems}
          disabled={loading !== null || importing !== null}
          onValueChange={id => {
            if (id === null) return
            const source = props.sourceBots.find(bot => bot.id === id)
            if (source !== undefined) {
              send(
                ImportFromBotEvents.SelectSource({
                  id: source.id,
                  name: source.name,
                  generation: source.generation,
                }),
              )
            }
          }}
        >
          <SelectTrigger id="import-from-bot-source" className="w-full">
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
        <SkillChecklist
          key={`${choiceState.source.id}@${choiceState.source.generation}`}
          sourceName={choiceState.source.name}
          skills={choiceState.skills.skills}
          currentSkills={props.currentSkills}
          error={choosing?.error ?? null}
          importing={importing !== null}
        />
      )}
      {imported === null ? null : (
        <ImportCompletion state={imported.skills} onImported={props.onImported} />
      )}
    </div>
  )
}

interface ImportFromBotControlProps {
  readonly scopeKey: string
  readonly targetBotId: string
  readonly targetBotGeneration: number | null
  readonly sourceBots: ReadonlyArray<ImportFromBotSource>
  readonly currentSkills: ReadonlyArray<BotSkill>
  readonly onImported: (state: BotSkillsState) => void
  readonly renderTrigger: (open: () => void) => ReactNode
}

function ImportFromBotTriggerAndDialog(props: ImportFromBotControlProps) {
  const machine = ImportFromBotOwner.useMachine()
  const send = useAtomSet(machine.send)
  const closed = AsyncResult.getOrElse(
    useMachineResult(AtomMachine.matches(machine, 'Closed')),
    () => true,
  )
  const open = !closed

  return (
    <>
      {props.renderTrigger(() => send(ImportFromBotEvents.Open()))}
      <Dialog
        open={open}
        onOpenChange={next => send(next ? ImportFromBotEvents.Open() : ImportFromBotEvents.Close())}
      >
        <DialogContent className="max-h-[calc(100dvh-4rem)] grid-cols-[minmax(0,1fr)] overflow-y-auto sm:max-w-xl">
          <DialogHeader className="sticky -top-4 z-10 -mx-4 -mt-4 bg-popover px-4 pt-4 pb-1">
            <DialogTitle>Import from bot</DialogTitle>
          </DialogHeader>
          {open ? (
            <ImportFromBotDialogBody
              sourceBots={props.sourceBots}
              currentSkills={props.currentSkills}
              onImported={props.onImported}
            />
          ) : null}
        </DialogContent>
      </Dialog>
    </>
  )
}

/** Dialog for copying selected skills from another bot into this bot. */
export function ImportFromBotControl(props: ImportFromBotControlProps) {
  return (
    <ImportFromBotOwner.Provider
      key={props.scopeKey}
      input={{
        targetBotId: props.targetBotId,
        targetBotGeneration: props.targetBotGeneration,
      }}
    >
      <ImportFromBotTriggerAndDialog {...props} />
    </ImportFromBotOwner.Provider>
  )
}
