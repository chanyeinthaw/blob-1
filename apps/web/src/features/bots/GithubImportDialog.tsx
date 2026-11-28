import type { BotSkillsState, GithubSkillPreview, GithubSkillsPreview } from '@blob-1/contracts'
import { useAtomSet } from '@effect/atom-react'
import { AtomMachine } from '@typeonce/effect-machine/reactivity'
import { createMachineContext, useMachineResult } from '@typeonce/effect-machine-react'
import { Option } from 'effect'
import { AsyncResult } from 'effect/reactivity'
import { Check, Download } from 'lucide-react'
import { type FormEvent, useEffect, useState } from 'react'

import { FieldError, FieldRow } from '~/components/layout/Page'
import { Button } from '~/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '~/components/ui/dialog'
import { Input } from '~/components/ui/input'
import { machineAtomsForBot } from '~/lib/runtime'
import { cn } from '~/lib/utils'

import { GithubImportEvents, GithubImportMachine } from './githubImportMachine'

const GithubImportOwner = createMachineContext(
  ({ input }: { readonly input: { readonly instanceKey: string } }) =>
    machineAtomsForBot(input.instanceKey).make(GithubImportMachine),
)

const filesLabel = (count: number): string => `${count} ${count === 1 ? 'file' : 'files'}`

interface ImportCompletionProps {
  readonly state: BotSkillsState
  readonly onImported: (state: BotSkillsState) => void
}

function ImportCompletion(props: ImportCompletionProps) {
  const machine = GithubImportOwner.useMachine()
  const send = useAtomSet(machine.send)
  const { onImported, state } = props

  useEffect(() => {
    onImported(state)
    send(GithubImportEvents.Close())
  }, [onImported, send, state])

  return null
}

interface ChooseSkillsStepProps {
  readonly preview: GithubSkillsPreview
  readonly error: string | null
  readonly importing: boolean
}

function ChooseSkillsStep(props: ChooseSkillsStepProps) {
  const machine = GithubImportOwner.useMachine()
  const send = useAtomSet(machine.send)
  const selectableFolders = props.preview.skills.map(skill => skill.folder)
  const [checked, setChecked] = useState<ReadonlySet<string>>(() => new Set(selectableFolders))

  const selectedFolders = props.preview.skills
    .filter(skill => checked.has(skill.folder))
    .map(skill => skill.folder)
  const allSelected =
    selectableFolders.length > 0 && selectableFolders.every(folder => checked.has(folder))

  const toggleSkill = (skill: GithubSkillPreview) => {
    setChecked(current => {
      const next = new Set(current)
      if (next.has(skill.folder)) {
        next.delete(skill.folder)
      } else {
        next.add(skill.folder)
      }
      return next
    })
    if (props.error !== null) send(GithubImportEvents.ClearError())
  }

  const toggleAll = () => {
    setChecked(allSelected ? new Set() : new Set(selectableFolders))
    if (props.error !== null) send(GithubImportEvents.ClearError())
  }

  return (
    <div className="space-y-3">
      <div className="flex items-center justify-between gap-3">
        <p className="min-w-0 truncate text-sm">
          <span className="font-medium">{props.preview.repo}</span>
          <span className="ml-2 font-mono text-xs text-muted-foreground">
            {props.preview.commit.slice(0, 7)}
          </span>
        </p>
        <Button
          type="button"
          variant="ghost"
          size="xs"
          disabled={props.importing || selectableFolders.length === 0}
          onClick={toggleAll}
        >
          {allSelected ? 'Select none' : 'Select all'}
        </Button>
      </div>
      <ul className="-mx-4 divide-y divide-border/60 border-t border-border/60">
        {props.preview.skills.map(skill => {
          const disabled = props.importing
          const isChecked = checked.has(skill.folder)
          return (
            <li key={skill.folder}>
              <label
                className={cn(
                  'flex w-full items-center gap-3 px-4 py-2.5 text-left transition-colors',
                  disabled ? 'cursor-not-allowed opacity-60' : 'cursor-pointer hover:bg-accent/60',
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
                    <span className="ml-auto shrink-0 text-xs text-muted-foreground">
                      {skill.conflict === null
                        ? null
                        : skill.conflict === 'detected'
                          ? 'Hides detected · '
                          : 'Replaces managed · '}
                      {filesLabel(skill.files)}
                    </span>
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
      {props.error === null ? null : <FieldError>{props.error}</FieldError>}
      <DialogFooter className="sticky -bottom-4 z-10 bg-popover">
        <Button
          type="button"
          variant="ghost"
          disabled={props.importing}
          onClick={() => send(GithubImportEvents.Back())}
        >
          Back
        </Button>
        <Button
          type="button"
          disabled={props.importing || selectedFolders.length === 0}
          onClick={() => send(GithubImportEvents.Import({ folders: selectedFolders }))}
        >
          {props.importing ? 'Importing…' : `Import ${selectedFolders.length}`}
        </Button>
      </DialogFooter>
    </div>
  )
}

/**
 * Splits a pasted `github.com/<owner>/<repo>/(tree|blob)/<ref>/<path…>` URL into
 * the repo URL with its ref and the folder path; `null` when there is no path.
 * A ref containing `/` may split wrong, but the server rejoins both fields.
 */
function splitGithubUrl(raw: string): { readonly url: string; readonly path: string } | null {
  const match =
    /^(https?:\/\/(?:www\.)?github\.com\/[^/\s]+\/[^/\s]+\/(?:tree|blob)\/[^/\s]+)\/(.+)$/.exec(
      raw.trim(),
    )
  if (match === null) return null
  const [, url = '', rest = ''] = match
  const path = rest.replace(/\/+$/, '').replace(/(^|\/)SKILL\.md$/, '')
  return path === '' ? null : { url: url.replace('/blob/', '/tree/'), path }
}

interface GithubUrlStepProps {
  readonly url: string
  readonly path: string
  readonly error: string | null
  readonly previewing: boolean
  readonly onUrlChange: (value: string) => void
  readonly onPathChange: (value: string) => void
}

function GithubUrlStep(props: GithubUrlStepProps) {
  const machine = GithubImportOwner.useMachine()
  const send = useAtomSet(machine.send)

  const editUrl = (value: string) => {
    const split = splitGithubUrl(value)
    props.onUrlChange(split?.url ?? value)
    if (split !== null) props.onPathChange(split.path)
    if (props.error !== null) send(GithubImportEvents.ClearError())
  }

  const editPath = (value: string) => {
    props.onPathChange(value)
    if (props.error !== null) send(GithubImportEvents.ClearError())
  }

  const submitPreview = (event: FormEvent) => {
    event.preventDefault()
    if (props.url.trim() === '') return
    send(GithubImportEvents.Preview({ url: props.url, path: props.path }))
  }

  return (
    <form noValidate onSubmit={submitPreview} className="space-y-2">
      <FieldRow id="github-url" label="GitHub URL">
        <Input
          id="github-url"
          className="font-mono"
          placeholder="https://github.com/owner/repo"
          value={props.url}
          disabled={props.previewing}
          onChange={event => editUrl(event.target.value)}
        />
      </FieldRow>
      <FieldRow id="github-path" label="Path">
        <Input
          id="github-path"
          className="font-mono"
          placeholder="skills"
          value={props.path}
          disabled={props.previewing}
          onChange={event => editPath(event.target.value)}
        />
      </FieldRow>
      {props.error === null ? null : <FieldError>{props.error}</FieldError>}
      <DialogFooter className="pt-2">
        <Button
          type="button"
          variant="ghost"
          disabled={props.previewing}
          onClick={() => send(GithubImportEvents.Close())}
        >
          Cancel
        </Button>
        <Button type="submit" disabled={props.previewing || props.url.trim() === ''}>
          {props.previewing ? 'Finding…' : 'Find skills'}
        </Button>
      </DialogFooter>
    </form>
  )
}

interface GithubImportDialogBodyProps {
  readonly onImported: (state: BotSkillsState) => void
}

function GithubImportDialogBody(props: GithubImportDialogBodyProps) {
  const machine = GithubImportOwner.useMachine()
  const [url, setUrl] = useState('')
  const [path, setPath] = useState('')

  const editing = Option.getOrNull(
    AsyncResult.getOrElse(useMachineResult(AtomMachine.select(machine, 'Editing')), () =>
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
  const previewing = AsyncResult.getOrElse(
    useMachineResult(AtomMachine.matches(machine, 'Previewing')),
    () => false,
  )

  const previewState = choosing ?? importing

  return (
    <>
      {previewState === null ? (
        <GithubUrlStep
          url={url}
          path={path}
          error={editing?.error ?? null}
          previewing={previewing}
          onUrlChange={setUrl}
          onPathChange={setPath}
        />
      ) : (
        <ChooseSkillsStep
          key={`${previewState.preview.repo}:${previewState.preview.commit}:${previewState.preview.path}`}
          preview={previewState.preview}
          error={choosing?.error ?? null}
          importing={importing !== null}
        />
      )}
      {imported === null ? null : (
        <ImportCompletion state={imported.skills} onImported={props.onImported} />
      )}
    </>
  )
}

interface GithubImportControlProps {
  readonly scopeKey: string
  readonly onImported: (state: BotSkillsState) => void
}

function GithubImportButtonAndDialog(props: GithubImportControlProps) {
  const machine = GithubImportOwner.useMachine()
  const send = useAtomSet(machine.send)
  const closed = AsyncResult.getOrElse(
    useMachineResult(AtomMachine.matches(machine, 'Closed')),
    () => true,
  )
  const open = !closed

  return (
    <>
      <Button
        type="button"
        variant="outline"
        size="sm"
        onClick={() => send(GithubImportEvents.Open())}
      >
        <Download data-icon="inline-start" />
        Import
      </Button>
      <Dialog
        open={open}
        onOpenChange={next => send(next ? GithubImportEvents.Open() : GithubImportEvents.Close())}
      >
        <DialogContent className="max-h-[calc(100dvh-4rem)] grid-cols-[minmax(0,1fr)] overflow-y-auto sm:max-w-xl">
          <DialogHeader className="sticky -top-4 z-10 -mx-4 -mt-4 bg-popover px-4 pt-4 pb-1">
            <DialogTitle>Import skills</DialogTitle>
          </DialogHeader>
          {open ? <GithubImportDialogBody onImported={props.onImported} /> : null}
        </DialogContent>
      </Dialog>
    </>
  )
}

/** Title-bar button and two-step dialog for importing skills from a GitHub repository. */
export function GithubImportControl(props: GithubImportControlProps) {
  return (
    <GithubImportOwner.Provider key={props.scopeKey} input={{ instanceKey: props.scopeKey }}>
      <GithubImportButtonAndDialog {...props} />
    </GithubImportOwner.Provider>
  )
}
