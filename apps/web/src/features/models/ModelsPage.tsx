import {
  type ModelInfo,
  type Models,
  type ProviderApi,
  type ProviderModels,
  Reasoning,
} from '@blob-1/contracts'
import { useAtomRefresh, useAtomSet, useAtomValue } from '@effect/atom-react'
import { Link } from '@tanstack/react-router'
import { MachineState } from '@typeonce/effect-machine-react'
import { ChevronDown, Search, X } from 'lucide-react'
import { useEffect, useState } from 'react'

import {
  EmptyLine,
  FieldRow,
  focusRingClassName,
  Page,
  PageError,
  Section,
  tableRowHoverClassName,
} from '~/components/layout/Page'
import { SplitDetail } from '~/components/layout/SplitPage'
import { Picker } from '~/components/Picker'
import { Button } from '~/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from '~/components/ui/dropdown-menu'
import { Input } from '~/components/ui/input'
import { Switch } from '~/components/ui/switch'
import { useBotInstanceKey, useBotsRoot } from '~/features/bots/CreateBotDialog'
import { machineAtomsForBot } from '~/lib/runtime'
import { createPersistentMachineContext } from '~/lib/shellMachines'
import { matchCached } from '~/lib/useBackgroundRefresh'
import { cn } from '~/lib/utils'

import {
  type ContextDraft,
  contextKey,
  contextText,
  ModelsEvents,
  type ModelsInput,
  ModelsMachine,
  parseContextLimit,
} from './modelsMachine'
import { modelsCatalogAtom, modelsPageAtomFor } from './modelsState'

const ModelsOwner = createPersistentMachineContext(
  'models',
  ({ input, scopeKey }: { readonly input: ModelsInput; readonly scopeKey: string }) =>
    machineAtomsForBot(scopeKey).make(ModelsMachine, input),
)

const PAGE_SIZE = 25
const ALL = '\0all'

interface Filters {
  readonly query: string
  readonly providerFilter: string
  readonly visibleCount: number
}

interface Entry {
  readonly provider: string
  readonly api: ProviderApi
  readonly info: ModelInfo
}

const entriesOf = (shown: Models): ReadonlyArray<Entry> =>
  shown.providers.flatMap(provider =>
    provider.models.map(info => ({ provider: provider.id, api: provider.api, info })),
  )

const isMatch = (filters: Filters, entry: Entry): boolean =>
  (filters.providerFilter === '' || entry.provider === filters.providerFilter) &&
  `${entry.info.id} ${entry.provider}`.toLowerCase().includes(filters.query.trim().toLowerCase())

const gridClassName = (showProvider: boolean): string =>
  showProvider
    ? 'sm:grid-cols-[minmax(0,1fr)_104px_132px_48px_48px]'
    : 'sm:grid-cols-[minmax(0,1fr)_132px_48px_48px]'

/** Global Model catalog: context limits and image capabilities across all providers. */
export function ModelsPage() {
  const result = useAtomValue(modelsCatalogAtom)
  const retry = useAtomRefresh(modelsCatalogAtom)
  return matchCached(result, {
    onInitial: () => (
      <Page title="Model catalog" width="wide">
        <EmptyLine>Loading…</EmptyLine>
      </Page>
    ),
    onFailure: failure => (
      <Page title="Model catalog" width="wide">
        <PageError cause={failure.cause} onRetry={retry} />
      </Page>
    ),
    onSuccess: value => (
      <ModelsOwner.Provider input={value} scopeKey="default@0">
        <ModelsLoaded />
      </ModelsOwner.Provider>
    ),
  })
}

interface BotModelsPageProps {
  readonly botId: string
}

/** Per-bot Models page: assigns Main, Compaction, Utility, and Decision roles from the shared catalog. */
export function BotModelsPage(props: BotModelsPageProps) {
  const instanceKey = useBotInstanceKey(props.botId)
  const atom = modelsPageAtomFor(instanceKey)
  const result = useAtomValue(atom)
  const retry = useAtomRefresh(atom)
  const bots = useBotsRoot()?.bots ?? []
  const botName = bots.find(entry => entry.id === props.botId)?.name ?? props.botId

  const wrap = (children: React.ReactNode) => (
    <SplitDetail
      title="Models"
      isListView={false}
      backTo="/settings/bots/$botId"
      backParams={{ botId: props.botId }}
      backLabel={botName}
      actions={
        <Button variant="outline" size="sm" render={<Link to="/settings/models" />}>
          Model catalog
        </Button>
      }
    >
      {children}
    </SplitDetail>
  )

  return matchCached(result, {
    onInitial: () => wrap(<EmptyLine>Loading…</EmptyLine>),
    onFailure: failure => wrap(<PageError cause={failure.cause} onRetry={retry} />),
    onSuccess: value => (
      <ModelsOwner.Provider input={value} scopeKey={instanceKey}>
        {wrap(<BotModelRoles />)}
      </ModelsOwner.Provider>
    ),
  })
}

const NONE_MODEL = '\0none'

const slotKeyOf = (slot: { readonly provider: string; readonly model: string } | null): string =>
  slot === null || slot.provider === '' || slot.model === ''
    ? NONE_MODEL
    : `${slot.provider}/${slot.model}`

const parseSlotKey = (
  value: string,
): { readonly provider: string; readonly model: string } | null => {
  if (value === NONE_MODEL) return null
  const slash = value.indexOf('/')
  if (slash <= 0) return null
  return { provider: value.slice(0, slash), model: value.slice(slash + 1) }
}

interface RoleOption {
  readonly value: string
  readonly label: string
  readonly hint: string | null
}

/** Keeps the assigned model visible in a picker even after it's disabled in the catalog. */
const withCurrent = (
  options: ReadonlyArray<RoleOption>,
  value: string,
): ReadonlyArray<RoleOption> =>
  value !== '' && value !== NONE_MODEL && options.every(option => option.value !== value)
    ? [...options, { value, label: value.slice(value.indexOf('/') + 1), hint: 'disabled' }]
    : options

function BotModelRoles() {
  const machine = ModelsOwner.useMachine()
  const send = useAtomSet(machine.send)

  useEffect(() => {
    send(ModelsEvents.Refresh())
  }, [send])

  return (
    <MachineState machine={machine} path="">
      {({ value }) => {
        const { shown, roleStatuses } = value
        const all = entriesOf(shown)
        const llmOptions = all
          .filter(entry => entry.api !== 'decision' && entry.info.enabled)
          .map(entry => ({
            value: `${entry.provider}/${entry.info.id}`,
            label: entry.info.id,
            hint: entry.provider,
          }))
        const decisionOptions = all
          .filter(entry => entry.api === 'decision' && entry.info.enabled)
          .map(entry => ({
            value: `${entry.provider}/${entry.info.id}`,
            label: entry.info.id,
            hint: entry.provider,
          }))
        const reasoningOptions = Reasoning.literals.map(level => ({
          value: level,
          label: level,
          hint: null,
        }))

        const mainValue = slotKeyOf(shown.main)
        const compactionValue = slotKeyOf(shown.compaction)
        const utilityValue = slotKeyOf(shown.utility)
        const decisionValue = slotKeyOf(shown.decision)

        return (
          <div className="space-y-6">
            {all.length === 0 ? (
              <EmptyLine>
                No models discovered yet.{' '}
                <Link to="/settings/providers" className="underline underline-offset-4">
                  Add a provider
                </Link>
              </EmptyLine>
            ) : (
              <Section title="Roles">
                <div className="divide-y divide-border/60">
                  <FieldRow id="bot-model-main" label="Main">
                    <div className="flex flex-wrap items-center gap-2">
                      <div className="min-w-0 flex-1">
                        <Picker
                          id="bot-model-main"
                          value={mainValue === NONE_MODEL ? '' : mainValue}
                          options={withCurrent(llmOptions, mainValue)}
                          placeholder="Choose a main model"
                          onChange={next => {
                            const parsed = parseSlotKey(next)
                            if (parsed === null) return
                            send(
                              ModelsEvents.AssignMain({
                                slot: {
                                  provider: parsed.provider,
                                  model: parsed.model,
                                  reasoning: shown.main.reasoning,
                                },
                              }),
                            )
                          }}
                        />
                      </div>
                      <div className="w-28 shrink-0">
                        <Picker
                          id="bot-model-main-reasoning"
                          value={shown.main.reasoning}
                          options={reasoningOptions}
                          placeholder="Reasoning"
                          disabled={mainValue === NONE_MODEL}
                          onChange={next => {
                            const level = Reasoning.literals.find(l => l === next)
                            if (level === undefined || mainValue === NONE_MODEL) return
                            send(
                              ModelsEvents.AssignMain({
                                slot: { ...shown.main, reasoning: level },
                              }),
                            )
                          }}
                        />
                      </div>
                    </div>
                    {roleStatuses.main ? (
                      <p className="mt-1 text-xs text-muted-foreground">{roleStatuses.main}</p>
                    ) : null}
                  </FieldRow>

                  <FieldRow id="bot-model-compaction" label="Compaction">
                    <div className="flex flex-wrap items-center gap-2">
                      <div className="min-w-0 flex-1">
                        <Picker
                          id="bot-model-compaction"
                          value={compactionValue}
                          options={[
                            { value: NONE_MODEL, label: 'Use main model', hint: null },
                            ...withCurrent(llmOptions, compactionValue),
                          ]}
                          placeholder="Use main model"
                          onChange={next => {
                            const parsed = parseSlotKey(next)
                            send(
                              ModelsEvents.AssignCompaction({
                                slot:
                                  parsed === null
                                    ? null
                                    : {
                                        provider: parsed.provider,
                                        model: parsed.model,
                                        reasoning: shown.compaction?.reasoning ?? 'medium',
                                      },
                              }),
                            )
                          }}
                        />
                      </div>
                      {shown.compaction !== null ? (
                        <div className="w-28 shrink-0">
                          <Picker
                            id="bot-model-compaction-reasoning"
                            value={shown.compaction.reasoning}
                            options={reasoningOptions}
                            placeholder="Reasoning"
                            onChange={next => {
                              const level = Reasoning.literals.find(l => l === next)
                              if (level === undefined || shown.compaction === null) return
                              send(
                                ModelsEvents.AssignCompaction({
                                  slot: { ...shown.compaction, reasoning: level },
                                }),
                              )
                            }}
                          />
                        </div>
                      ) : null}
                    </div>
                    {roleStatuses.compaction ? (
                      <p className="mt-1 text-xs text-muted-foreground">
                        {roleStatuses.compaction}
                      </p>
                    ) : null}
                  </FieldRow>

                  <FieldRow id="bot-model-utility" label="Utility">
                    <div className="flex flex-wrap items-center gap-2">
                      <div className="min-w-0 flex-1">
                        <Picker
                          id="bot-model-utility"
                          value={utilityValue}
                          options={[
                            { value: NONE_MODEL, label: 'Off', hint: null },
                            ...withCurrent(llmOptions, utilityValue),
                          ]}
                          placeholder="Off"
                          onChange={next => {
                            const parsed = parseSlotKey(next)
                            send(
                              ModelsEvents.AssignUtility({
                                slot:
                                  parsed === null
                                    ? null
                                    : {
                                        provider: parsed.provider,
                                        model: parsed.model,
                                        reasoning: shown.utility?.reasoning ?? 'low',
                                      },
                              }),
                            )
                          }}
                        />
                      </div>
                      {shown.utility !== null ? (
                        <div className="w-28 shrink-0">
                          <Picker
                            id="bot-model-utility-reasoning"
                            value={shown.utility.reasoning}
                            options={reasoningOptions}
                            placeholder="Reasoning"
                            onChange={next => {
                              const level = Reasoning.literals.find(l => l === next)
                              if (level === undefined || shown.utility === null) return
                              send(
                                ModelsEvents.AssignUtility({
                                  slot: { ...shown.utility, reasoning: level },
                                }),
                              )
                            }}
                          />
                        </div>
                      ) : null}
                    </div>
                    {roleStatuses.utility ? (
                      <p className="mt-1 text-xs text-muted-foreground">{roleStatuses.utility}</p>
                    ) : null}
                  </FieldRow>

                  <FieldRow id="bot-model-decision" label="Decision">
                    <Picker
                      id="bot-model-decision"
                      value={decisionValue}
                      options={[
                        { value: NONE_MODEL, label: 'Off', hint: null },
                        ...withCurrent(decisionOptions, decisionValue),
                      ]}
                      placeholder="Off"
                      onChange={next => {
                        const parsed = parseSlotKey(next)
                        send(
                          ModelsEvents.AssignDecision({
                            slot: parsed,
                          }),
                        )
                      }}
                    />
                    {roleStatuses.decision ? (
                      <p className="mt-1 text-xs text-muted-foreground">{roleStatuses.decision}</p>
                    ) : null}
                  </FieldRow>
                </div>
              </Section>
            )}
          </div>
        )
      }}
    </MachineState>
  )
}

function ModelsLoaded() {
  const machine = ModelsOwner.useMachine()
  const send = useAtomSet(machine.send)
  useEffect(() => {
    send(ModelsEvents.Refresh())
  }, [send])
  const [filters, setFilters] = useState<Filters>({
    query: '',
    providerFilter: '',
    visibleCount: PAGE_SIZE,
  })
  const apply = (change: Partial<Filters>) =>
    setFilters(current => ({ ...current, ...change, visibleCount: PAGE_SIZE }))
  const showMore = () =>
    setFilters(current => ({ ...current, visibleCount: current.visibleCount + PAGE_SIZE }))
  return (
    <Page
      title="Model catalog"
      width="wide"
      wideActions
      actions={<FilterBar filters={filters} onChange={apply} />}
    >
      <Catalog filters={filters} onShowMore={showMore} />
    </Page>
  )
}

interface FilterBarProps {
  readonly filters: Filters
  readonly onChange: (change: Partial<Filters>) => void
}

function FilterBar(props: FilterBarProps) {
  const machine = ModelsOwner.useMachine()
  return (
    <div className="flex w-full flex-wrap items-center gap-2 md:w-auto md:flex-nowrap">
      <div className="basis-full sm:basis-auto sm:flex-1 md:w-[220px] md:flex-none">
        <div className="relative">
          <span className="pointer-events-none absolute inset-y-0 left-3 flex items-center text-muted-foreground">
            <Search className="size-4" />
          </span>
          <Input
            aria-label="Search models"
            placeholder="Search models"
            className={cn('bg-background pl-9 shadow-xs', props.filters.query !== '' && 'pr-9')}
            value={props.filters.query}
            onChange={event => props.onChange({ query: event.target.value })}
          />
          {props.filters.query === '' ? null : (
            <button
              type="button"
              aria-label="Clear search"
              onClick={() => props.onChange({ query: '' })}
              className={cn(
                'absolute inset-y-0 right-1 my-auto grid size-8 cursor-pointer place-items-center rounded-full text-muted-foreground hover:bg-muted hover:text-foreground md:right-1.5 md:size-6',
                focusRingClassName,
              )}
            >
              <X className="size-3.5" />
            </button>
          )}
        </div>
      </div>
      <MachineState machine={machine} path="">
        {({ value }) => (
          <ProviderFilter
            providers={value.shown.providers}
            selected={props.filters.providerFilter}
            onChange={providerFilter => props.onChange({ providerFilter })}
          />
        )}
      </MachineState>
    </div>
  )
}

interface ProviderFilterProps {
  readonly providers: ReadonlyArray<ProviderModels>
  readonly selected: string
  readonly onChange: (provider: string) => void
}

function ProviderFilter(props: ProviderFilterProps) {
  const total = props.providers.reduce((count, provider) => count + provider.models.length, 0)
  const label = props.selected === '' ? 'All providers' : props.selected
  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        render={
          <Button
            variant="outline"
            aria-label={`Provider: ${label}`}
            className="min-w-0 flex-1 justify-between sm:w-40 sm:flex-none md:w-44"
          >
            <span className="truncate">{label}</span>
            <ChevronDown className="size-3.5 shrink-0 text-muted-foreground" />
          </Button>
        }
      />
      <DropdownMenuContent align="end" className="w-48">
        <DropdownMenuRadioGroup
          value={props.selected === '' ? ALL : props.selected}
          onValueChange={value => props.onChange(value === ALL ? '' : value)}
        >
          <DropdownMenuRadioItem value={ALL}>
            <span className="truncate">All providers</span>
            <span className="ml-auto shrink-0 pl-3 text-xs text-muted-foreground tabular-nums">
              {total}
            </span>
          </DropdownMenuRadioItem>
          {props.providers.map(provider => (
            <DropdownMenuRadioItem key={provider.id} value={provider.id}>
              <span className="truncate">{provider.id}</span>
              <span className="ml-auto shrink-0 pl-3 text-xs text-muted-foreground tabular-nums">
                {provider.models.length}
              </span>
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

interface CatalogProps {
  readonly filters: Filters
  readonly onShowMore: () => void
}

function Catalog(props: CatalogProps) {
  const machine = ModelsOwner.useMachine()
  return (
    <MachineState machine={machine} path="">
      {({ value }) => {
        const all = entriesOf(value.shown)
        const matching = all.filter(entry => isMatch(props.filters, entry))
        const visible = matching.slice(0, props.filters.visibleCount)
        const remaining = matching.length - visible.length
        const showProvider = props.filters.providerFilter === ''
        return (
          <div>
            {matching.length === 0 ? (
              <EmptyLine>
                {all.length === 0 ? 'No models available.' : 'No models match.'}
              </EmptyLine>
            ) : (
              <div>
                <ColumnHeader showProvider={showProvider} />
                <ul className="flex flex-col divide-y">
                  {visible.map(entry => (
                    <ModelRow
                      key={`${entry.provider}/${entry.info.id}`}
                      entry={entry}
                      draft={value.contextDrafts[contextKey(entry.provider, entry.info.id)]}
                      showProvider={showProvider}
                    />
                  ))}
                </ul>
              </div>
            )}
            {remaining > 0 ? (
              <div className="mt-3">
                <Button variant="secondary" onClick={props.onShowMore}>
                  Show {Math.min(PAGE_SIZE, remaining)} more
                </Button>
              </div>
            ) : null}
          </div>
        )
      }}
    </MachineState>
  )
}

interface ColumnHeaderProps {
  readonly showProvider: boolean
}

function ColumnHeader(props: ColumnHeaderProps) {
  return (
    <div
      className={cn(
        'hidden h-10 items-center gap-x-3 border-b border-border px-2 text-sm font-medium text-muted-foreground sm:grid',
        gridClassName(props.showProvider),
      )}
    >
      <span>Model</span>
      {props.showProvider ? <span>Provider</span> : null}
      <span className="text-right whitespace-nowrap">Context</span>
      <span className="text-right">Images</span>
      <span className="text-right">On</span>
    </div>
  )
}

interface ModelRowProps {
  readonly entry: Entry
  readonly draft: ContextDraft | undefined
  readonly showProvider: boolean
}

function ModelRow(props: ModelRowProps) {
  return (
    <li
      className={cn(
        'group grid grid-cols-1 items-center gap-x-3 gap-y-2.5 px-1 py-3 sm:min-h-12 sm:gap-y-1.5 sm:px-2 sm:py-2',
        tableRowHoverClassName,
        gridClassName(props.showProvider),
      )}
    >
      <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1.5">
        <span
          className={cn(
            'min-w-0 break-words font-mono text-sm [overflow-wrap:anywhere] sm:truncate',
            !props.entry.info.enabled && 'text-muted-foreground',
          )}
        >
          {props.entry.info.id}
        </span>
        {props.showProvider ? (
          <span className="text-xs text-muted-foreground sm:hidden">{props.entry.provider}</span>
        ) : null}
      </div>
      <div className="flex flex-wrap items-start gap-x-2 gap-y-2 sm:contents">
        {props.showProvider ? (
          <span className="hidden min-w-0 truncate text-sm text-muted-foreground sm:block">
            {props.entry.provider}
          </span>
        ) : null}
        <ContextInput entry={props.entry} draft={props.draft} />
        <ImagesSwitch provider={props.entry.provider} info={props.entry.info} />
        <EnabledSwitch provider={props.entry.provider} info={props.entry.info} />
      </div>
    </li>
  )
}

interface FlagSwitchProps {
  readonly provider: string
  readonly info: ModelInfo
}

function ImagesSwitch(props: FlagSwitchProps) {
  const machine = ModelsOwner.useMachine()
  const send = useAtomSet(machine.send)
  const switchId = `images-${encodeURIComponent(JSON.stringify([props.provider, props.info.id]))}`
  return (
    <div className="flex h-10 shrink-0 items-center justify-end gap-1.5 sm:h-auto sm:gap-2">
      <label
        htmlFor={switchId}
        className="cursor-pointer text-xs font-medium text-muted-foreground sm:hidden"
      >
        Images
      </label>
      <Switch
        id={switchId}
        aria-label={`Images for ${props.info.id}`}
        checked={props.info.images}
        onCheckedChange={checked =>
          send(
            ModelsEvents.ToggleImages({
              provider: props.provider,
              modelId: props.info.id,
              images: checked,
            }),
          )
        }
      />
    </div>
  )
}

function EnabledSwitch(props: FlagSwitchProps) {
  const machine = ModelsOwner.useMachine()
  const send = useAtomSet(machine.send)
  const switchId = `enabled-${encodeURIComponent(JSON.stringify([props.provider, props.info.id]))}`
  return (
    <div className="flex h-10 shrink-0 items-center justify-end gap-1.5 sm:h-auto sm:gap-2">
      <label
        htmlFor={switchId}
        className="cursor-pointer text-xs font-medium text-muted-foreground sm:hidden"
      >
        Enabled
      </label>
      <Switch
        id={switchId}
        aria-label={`Enabled for ${props.info.id}`}
        checked={props.info.enabled}
        onCheckedChange={checked =>
          send(
            ModelsEvents.ToggleEnabled({
              provider: props.provider,
              modelId: props.info.id,
              enabled: checked,
            }),
          )
        }
      />
    </div>
  )
}

const isInvalidStatus = (status: string): boolean =>
  status !== '' && status !== 'Saved' && status !== 'Saving…'

const isCappedOverride = (info: ModelInfo, value: string): boolean => {
  const parsed = parseContextLimit(value)
  return parsed !== undefined && parsed !== null && info.context !== null && parsed > info.context
}

interface ContextInputProps {
  readonly entry: Entry
  readonly draft: ContextDraft | undefined
}

function ContextInput(props: ContextInputProps) {
  const machine = ModelsOwner.useMachine()
  const send = useAtomSet(machine.send)
  const { provider, info } = props.entry
  const value = props.draft?.value ?? (info.limit === null ? '' : contextText(info.limit))
  const status = props.draft?.status ?? ''
  const key = encodeURIComponent(JSON.stringify([provider, info.id]))
  const inputId = `context-input-${key}`
  const descriptionId = `context-${key}`
  const flush = () => send(ModelsEvents.FlushContext({ provider, modelId: info.id }))
  const contextLabel =
    info.limit === null
      ? `Context limit for ${info.id} (inherited: ${contextText(info.context)})`
      : `Context limit for ${info.id} (custom override)`
  return (
    <div className="flex shrink-0 flex-col items-end">
      <div className="flex items-center gap-1.5 sm:gap-2">
        <label
          htmlFor={inputId}
          className="cursor-pointer text-xs font-medium text-muted-foreground sm:hidden"
        >
          Context
        </label>
        <Input
          id={inputId}
          aria-label={contextLabel}
          aria-invalid={isInvalidStatus(status)}
          aria-describedby={descriptionId}
          inputMode="numeric"
          placeholder={contextText(info.context)}
          value={value}
          onChange={event =>
            send(
              ModelsEvents.UpdateContext({
                provider,
                modelId: info.id,
                value: event.target.value,
              }),
            )
          }
          onKeyDown={event => {
            if (event.key === 'Enter') {
              event.preventDefault()
              flush()
            }
          }}
          onBlur={flush}
          className={cn(
            'h-10 w-[120px] px-2.5 text-right font-mono text-base tabular-nums sm:h-7 sm:w-[116px] sm:px-3 sm:text-sm',
            value === '' ? 'bg-secondary/70' : 'bg-secondary',
          )}
        />
      </div>
      <ContextStatus id={descriptionId} info={info} value={value} status={status} />
    </div>
  )
}

interface ContextStatusProps {
  readonly id: string
  readonly info: ModelInfo
  readonly value: string
  readonly status: string
}

function ContextStatus(props: ContextStatusProps) {
  const capped = isCappedOverride(props.info, props.value)
  const visibleStatus = props.status === 'Saved' ? '' : props.status
  const text = capped
    ? `Max ${contextText(props.info.context)}${visibleStatus === '' ? '' : ` · ${visibleStatus}`}`
    : visibleStatus
  return (
    <span
      id={props.id}
      aria-live="polite"
      className={cn(
        'mt-1 max-w-[120px] break-words text-right text-xs [overflow-wrap:anywhere] empty:sr-only sm:max-w-[132px]',
        isInvalidStatus(props.status) ? 'text-destructive' : 'text-muted-foreground',
      )}
    >
      {text}
    </span>
  )
}
