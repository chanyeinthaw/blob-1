import { useEffect, useRef, useState } from 'react'

import {
  Combobox,
  ComboboxCollection,
  ComboboxContent,
  ComboboxEmpty,
  ComboboxGroup,
  ComboboxInput,
  ComboboxItem,
  ComboboxLabel,
  ComboboxList,
  useComboboxAnchor,
} from '~/components/ui/combobox'

export interface PickerOption {
  readonly value: string
  readonly label: string
  readonly hint: string | null
  /** Section heading this option belongs to, e.g. a Discord server. */
  readonly group?: string | null
}

type PickerOptionGroup = {
  readonly key: string
  readonly label: string | null
  readonly items: ReadonlyArray<PickerOption>
}

interface PickerProps {
  readonly id: string
  readonly options: ReadonlyArray<PickerOption>
  readonly value: string
  readonly placeholder: string
  readonly focusOnMount?: boolean
  readonly disabled?: boolean
  readonly clearOnSelect?: boolean
  readonly clearOnEdit?: boolean
  readonly customOption?: (query: string) => PickerOption | null
  readonly emptyMessage?: (query: string) => string
  readonly onChange: (value: string) => void
}

const matchesOption = (option: PickerOption, query: string): boolean => {
  const needle = query.trim().toLowerCase()
  if (needle === '') return true
  return (
    option.value.toLowerCase() === needle ||
    option.label.toLowerCase().includes(needle) ||
    (option.hint !== null && option.hint.toLowerCase().includes(needle)) ||
    (option.group !== undefined &&
      option.group !== null &&
      option.group.toLowerCase().includes(needle))
  )
}

const resolveOptions = (
  options: ReadonlyArray<PickerOption>,
  value: string,
  custom: PickerOption | null,
) => {
  const known = options.find(option => option.value === value)
  const fallback: PickerOption | null =
    known === undefined && value !== '' ? { value, label: value, hint: null } : null
  const base = fallback === null ? options : [fallback, ...options]
  const items =
    custom !== null && !base.some(option => option.value === custom.value)
      ? [...base, custom]
      : base
  return { selected: known ?? fallback, items }
}

/**
 * Splits items into contiguous runs by `group` when they span two or more
 * distinct groups; returns null for a flat list. Runs keep option order, so
 * ungrouped entries (saved-value fallback, custom option) stay in place.
 */
const toOptionGroups = (
  items: ReadonlyArray<PickerOption>,
): ReadonlyArray<PickerOptionGroup> | null => {
  const distinct = new Set<string>()
  for (const item of items) {
    if (item.group !== undefined && item.group !== null) distinct.add(item.group)
  }
  if (distinct.size < 2) return null
  const runs: { label: string | null; items: Array<PickerOption> }[] = []
  for (const item of items) {
    const label = item.group ?? null
    const last = runs[runs.length - 1]
    if (last === undefined || last.label !== label) {
      runs.push({ label, items: [item] })
    } else {
      last.items.push(item)
    }
  }
  return runs.map((run, index) => ({
    key: `${index}:${run.label ?? ''}`,
    label: run.label,
    items: run.items,
  }))
}

const renderItem = (option: PickerOption) => (
  <ComboboxItem key={option.value} value={option}>
    <span className="min-w-0 flex-1 truncate">{option.label}</span>
    {option.hint === null ? null : (
      <span className="ml-auto max-w-[45%] shrink-0 truncate pl-2 text-xs text-muted-foreground">
        {option.hint}
      </span>
    )}
  </ComboboxItem>
)

/** A searchable single choice, e.g. a Discord channel or person. */
export function Picker(props: PickerProps) {
  const inputRef = useRef<HTMLInputElement>(null)
  const anchorRef = useComboboxAnchor()
  const initialLabel = resolveOptions(props.options, props.value, null).selected?.label ?? ''
  const [inputValue, setInputValue] = useState(initialLabel)
  const syncedRef = useRef({ value: props.value, label: initialLabel })
  const custom = props.customOption?.(inputValue) ?? null
  const { selected, items } = resolveOptions(props.options, props.value, custom)
  const groups = toOptionGroups(items)
  const resolvedLabel = selected?.label ?? ''

  useEffect(() => {
    if (props.focusOnMount === true) inputRef.current?.focus()
  }, [props.focusOnMount])

  useEffect(() => {
    const prev = syncedRef.current
    if (prev.value !== props.value) {
      syncedRef.current = { value: props.value, label: resolvedLabel }
      setInputValue(resolvedLabel)
    } else if (prev.label !== resolvedLabel) {
      syncedRef.current = { value: props.value, label: resolvedLabel }
      setInputValue(current => (current === prev.label ? resolvedLabel : current))
    }
  }, [props.value, resolvedLabel])

  const emptyText = props.emptyMessage?.(inputValue) ?? 'No matches'

  return (
    <Combobox
      items={groups ?? items}
      value={selected}
      inputValue={inputValue}
      filter={matchesOption}
      onInputValueChange={(next, details) => {
        if (props.clearOnSelect === true && details.reason === 'item-press') {
          setInputValue('')
          return
        }
        setInputValue(next)
        if (
          props.clearOnEdit === true &&
          (details.reason === 'input-change' || details.reason === 'input-clear') &&
          props.value !== '' &&
          next !== resolvedLabel
        ) {
          syncedRef.current = { value: '', label: '' }
          props.onChange('')
        }
      }}
      itemToStringLabel={(option: PickerOption) => option.label}
      isItemEqualToValue={(item: PickerOption, value: PickerOption) => item.value === value.value}
      onValueChange={(option: PickerOption | null) => {
        if (option !== null) {
          setInputValue(props.clearOnSelect === true ? '' : option.label)
          props.onChange(option.value)
        }
      }}
    >
      <div ref={anchorRef} className="w-full">
        <ComboboxInput
          ref={inputRef}
          id={props.id}
          placeholder={props.placeholder}
          disabled={props.disabled}
          className="w-full"
        />
      </div>
      <ComboboxContent anchor={anchorRef}>
        <ComboboxEmpty>{emptyText}</ComboboxEmpty>
        <ComboboxList>
          {groups === null
            ? renderItem
            : (group: PickerOptionGroup) => (
                <ComboboxGroup key={group.key} items={group.items}>
                  {group.label === null ? null : <ComboboxLabel>{group.label}</ComboboxLabel>}
                  <ComboboxCollection>{renderItem}</ComboboxCollection>
                </ComboboxGroup>
              )}
        </ComboboxList>
      </ComboboxContent>
    </Combobox>
  )
}
