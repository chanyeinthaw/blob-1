import type { BotSkill, BotSkillMode, BotSkillsState } from '@blob-1/contracts'
import { useAtomRefresh, useAtomSet, useAtomValue } from '@effect/atom-react'
import { MachineState } from '@typeonce/effect-machine-react'
import { ChevronDown, Download, FolderSearch, Package, PackagePlus, Trash2 } from 'lucide-react'
import { useCallback, useEffect, useState, type ReactNode } from 'react'

import { ConfirmDialog } from '~/components/ConfirmDialog'
import { EmptyLine, PageError, tableRowHoverClassName } from '~/components/layout/Page'
import { SplitDetail } from '~/components/layout/SplitPage'
import { Button } from '~/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '~/components/ui/dropdown-menu'
import { Input } from '~/components/ui/input'
import { Switch } from '~/components/ui/switch'
import { Tooltip, TooltipContent, TooltipTrigger } from '~/components/ui/tooltip'
import { machineAtomsForBot } from '~/lib/runtime'
import { createPersistentMachineContext } from '~/lib/shellMachines'
import { cn } from '~/lib/utils'
import { matchCached } from '~/lib/useBackgroundRefresh'

import { BotSkillsEvents, type BotSkillsInput, BotSkillsMachine } from './botSkillsMachine'
import { botSkillsAtomFor } from './botsState'
import { GithubImportControl } from './GithubImportDialog'
import { ImportFromBotControl } from './ImportFromBotDialog'
import type { ImportFromBotSource } from './importFromBotMachine'
import { SkillDetailSheet } from './SkillDetailSheet'
import { useBotInstanceKey, useBotsRoot } from './CreateBotDialog'

/** Skill | Chat | Task | actions, shared by the header and every row. */
const gridClassName = 'grid grid-cols-[minmax(0,1fr)_48px_48px_64px] items-center gap-x-3'

/** The stored mode with chat or task use turned on or off. */
function withUse(mode: BotSkillMode, use: 'chat' | 'task', on: boolean): BotSkillMode {
  const chat = use === 'chat' ? on : mode === 'both' || mode === 'chat'
  const task = use === 'task' ? on : mode === 'both' || mode === 'task'
  return chat && task ? 'both' : chat ? 'chat' : task ? 'task' : 'off'
}

const BotSkillsOwner = createPersistentMachineContext(
  'bot-skills',
  ({ input, scopeKey }: { readonly input: BotSkillsInput; readonly scopeKey: string }) =>
    machineAtomsForBot(scopeKey).make(BotSkillsMachine, input),
)

interface SkillRowProps {
  readonly instanceKey: string
  readonly skill: BotSkill
  readonly disabled: boolean
  readonly onModeChange: (mode: BotSkillMode) => void
  readonly onAdopt: () => void
  readonly onRemove: () => void
}

function SkillRow(props: SkillRowProps) {
  const { skill } = props
  const managed = skill.source === 'managed'
  const action = managed ? 'Remove' : 'Delete'
  return (
    <li className={cn(gridClassName, 'min-h-12 px-2 py-2', tableRowHoverClassName)}>
      <div className="flex min-w-0 items-center gap-2.5">
        <Tooltip>
          <TooltipTrigger
            render={
              <span
                className={cn(
                  'grid shrink-0 place-items-center',
                  managed ? 'text-foreground' : 'text-muted-foreground',
                )}
              >
                {managed ? <Package className="size-4" /> : <FolderSearch className="size-4" />}
                <span className="sr-only">{managed ? 'Managed' : 'Detected'}</span>
              </span>
            }
          />
          <TooltipContent>{managed ? 'Managed' : 'Detected'}</TooltipContent>
        </Tooltip>
        <SkillDetailSheet instanceKey={props.instanceKey} skill={skill} />
      </div>
      <Switch
        aria-label={`Bot can use ${skill.name}`}
        className="justify-self-end"
        checked={skill.mode === 'both' || skill.mode === 'chat'}
        disabled={props.disabled}
        onCheckedChange={on => props.onModeChange(withUse(skill.mode, 'chat', on))}
      />
      <Switch
        aria-label={`Tasks can use ${skill.name}`}
        className="justify-self-end"
        checked={skill.mode === 'both' || skill.mode === 'task'}
        disabled={props.disabled}
        onCheckedChange={on => props.onModeChange(withUse(skill.mode, 'task', on))}
      />
      <div className="flex items-center justify-end gap-1">
        {managed ? null : (
          <Tooltip>
            <TooltipTrigger
              render={
                <Button
                  variant="ghost"
                  size="icon-xs"
                  aria-label={`Make ${skill.name} managed`}
                  className="text-muted-foreground"
                  disabled={props.disabled}
                  onClick={props.onAdopt}
                >
                  <PackagePlus />
                </Button>
              }
            />
            <TooltipContent>Make managed</TooltipContent>
          </Tooltip>
        )}
        <ConfirmDialog
          tooltip={`${action} skill`}
          trigger={
            <Button
              variant="ghost"
              size="icon-xs"
              aria-label={`${action} ${skill.name}`}
              className="text-muted-foreground hover:bg-destructive/10 hover:text-destructive"
              disabled={props.disabled}
            >
              <Trash2 />
            </Button>
          }
          title={`${action} ${skill.name}?`}
          detail={
            managed
              ? 'Its files are deleted from this bot.'
              : "Its folder is deleted from the bot's machine."
          }
          confirmLabel={action}
          onConfirm={props.onRemove}
        />
      </div>
    </li>
  )
}

function SkillsHeader() {
  return (
    <div
      className={cn(
        gridClassName,
        'h-10 border-b border-border px-2 text-sm font-medium text-muted-foreground',
      )}
    >
      <span>Skill</span>
      <span className="text-right">Bot</span>
      <span className="text-right">Task</span>
      <span />
    </div>
  )
}

interface BotSkillsContentProps {
  readonly instanceKey: string
  readonly query: string
}

function BotSkillsContent(props: BotSkillsContentProps) {
  const machine = BotSkillsOwner.useMachine()
  const send = useAtomSet(machine.send)

  useEffect(() => {
    send(BotSkillsEvents.Refresh())
  }, [send])

  return (
    <MachineState machine={machine} path="">
      {({ value, states }) => {
        const allSkills = value.data.skills
        if (allSkills.length === 0) {
          return <EmptyLine>No skills found.</EmptyLine>
        }
        const query = props.query.trim().toLowerCase()
        const skills =
          query === ''
            ? allSkills
            : allSkills.filter(
                skill =>
                  skill.name.toLowerCase().includes(query) ||
                  skill.description.toLowerCase().includes(query),
              )
        if (skills.length === 0) {
          return <EmptyLine>No skills match.</EmptyLine>
        }
        const isMutating = states.Mutate.state.path === 'Mutate.Working'
        return (
          <div>
            <SkillsHeader />
            <ul className="flex flex-col divide-y">
              {skills.map(skill => (
                <SkillRow
                  key={skill.name}
                  instanceKey={props.instanceKey}
                  skill={skill}
                  disabled={isMutating}
                  onModeChange={mode => send(BotSkillsEvents.SetMode({ name: skill.name, mode }))}
                  onAdopt={() => send(BotSkillsEvents.Adopt({ name: skill.name }))}
                  onRemove={() => send(BotSkillsEvents.Remove({ name: skill.name }))}
                />
              ))}
            </ul>
          </div>
        )
      }}
    </MachineState>
  )
}

interface ImportMenuProps {
  readonly instanceKey: string
  readonly botId: string
  readonly botGeneration: number | null
  readonly sourceBots: ReadonlyArray<ImportFromBotSource>
  readonly currentSkills: ReadonlyArray<BotSkill>
  readonly onImported: (state: BotSkillsState) => void
}

function ImportMenu(props: ImportMenuProps) {
  return (
    <GithubImportControl
      scopeKey={props.instanceKey}
      onImported={props.onImported}
      renderTrigger={openGithub => (
        <ImportFromBotControl
          scopeKey={props.instanceKey}
          targetBotId={props.botId}
          targetBotGeneration={props.botGeneration}
          sourceBots={props.sourceBots}
          currentSkills={props.currentSkills}
          onImported={props.onImported}
          renderTrigger={openBot => (
            <DropdownMenu>
              <DropdownMenuTrigger
                render={
                  <Button type="button" variant="outline" size="sm">
                    <Download data-icon="inline-start" />
                    Import
                    <ChevronDown data-icon="inline-end" className="text-muted-foreground" />
                  </Button>
                }
              />
              <DropdownMenuContent align="end" className="w-44">
                <DropdownMenuItem onClick={openGithub}>From GitHub…</DropdownMenuItem>
                {props.sourceBots.length === 0 ? null : (
                  <DropdownMenuItem onClick={openBot}>From another bot…</DropdownMenuItem>
                )}
              </DropdownMenuContent>
            </DropdownMenu>
          )}
        />
      )}
    />
  )
}

interface ImportSkillsActionProps {
  readonly instanceKey: string
  readonly botId: string
  readonly botGeneration: number | null
  readonly sourceBots: ReadonlyArray<ImportFromBotSource>
}

function ImportSkillsAction(props: ImportSkillsActionProps) {
  const machine = BotSkillsOwner.useMachine()
  const send = useAtomSet(machine.send)
  const handleImported = useCallback(
    (state: BotSkillsState) => send(BotSkillsEvents.ReplaceData({ state })),
    [send],
  )
  return (
    <MachineState machine={machine} path="">
      {({ value }) => (
        <ImportMenu
          instanceKey={props.instanceKey}
          botId={props.botId}
          botGeneration={props.botGeneration}
          sourceBots={props.sourceBots}
          currentSkills={value.data.skills}
          onImported={handleImported}
        />
      )}
    </MachineState>
  )
}

interface BotSkillsFrameProps {
  readonly botId: string
  readonly botName: string
  readonly query: string
  readonly action?: ReactNode
  readonly onQueryChange: (query: string) => void
  readonly children: ReactNode
}

function BotSkillsFrame(props: BotSkillsFrameProps) {
  return (
    <SplitDetail
      title="Skills"
      width="full"
      isListView={false}
      backTo="/settings/bots/$botId"
      backParams={{ botId: props.botId }}
      backLabel={props.botName}
      actions={
        <>
          <Input
            aria-label="Search skills"
            placeholder="Search skills"
            value={props.query}
            onChange={event => props.onQueryChange(event.target.value)}
            className="w-40 md:w-64"
          />
          {props.action}
        </>
      }
    >
      {props.children}
    </SplitDetail>
  )
}

interface BotSkillsPageProps {
  readonly botId: string
}

/** Per-bot skill list with optimistic access-mode, removal, adoption, and skill imports. */
export function BotSkillsPage(props: BotSkillsPageProps) {
  const bots = useBotsRoot()?.bots ?? []
  const bot = bots.find(entry => entry.id === props.botId)
  const botName = bot?.name ?? props.botId
  const [query, setQuery] = useState('')
  const instanceKey = useBotInstanceKey(props.botId)
  const atom = botSkillsAtomFor(instanceKey)
  const result = useAtomValue(atom)
  const retry = useAtomRefresh(atom)

  const frame = (children: ReactNode, action?: ReactNode) => (
    <BotSkillsFrame
      botId={props.botId}
      botName={botName}
      query={query}
      action={action}
      onQueryChange={setQuery}
    >
      {children}
    </BotSkillsFrame>
  )

  return matchCached(result, {
    onInitial: () => frame(<EmptyLine>Loading…</EmptyLine>),
    onFailure: failure => frame(<PageError cause={failure.cause} onRetry={retry} />),
    onSuccess: value => (
      <BotSkillsOwner.Provider input={value} scopeKey={instanceKey}>
        {frame(
          <BotSkillsContent instanceKey={instanceKey} query={query} />,
          <ImportSkillsAction
            instanceKey={instanceKey}
            botId={props.botId}
            botGeneration={bot?.generation ?? null}
            sourceBots={bots.filter(entry => entry.id !== props.botId)}
          />,
        )}
      </BotSkillsOwner.Provider>
    ),
  })
}
