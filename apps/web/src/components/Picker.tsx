import { useEffect, useRef, useState } from 'react'

import {
  Combobox,
  ComboboxContent,
  ComboboxEmpty,
  ComboboxInput,
  ComboboxItem,
  ComboboxList,
} from '~/components/ui/combobox'

export interface PickerOption {
  readonly value: string
  readonly label: string
  readonly hint: string | null
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
    option.label.toLowerCase().includes(needle) ||
    (option.hint !== null && option.hint.toLowerCase().includes(needle))
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

/** A searchable single choice, e.g. a Discord channel or person. */
export function Picker(props: PickerProps) {
  const inputRef = useRef<HTMLInputElement>(null)
  const initialLabel = resolveOptions(props.options, props.value, null).selected?.label ?? ''
  const [inputValue, setInputValue] = useState(initialLabel)
  const syncedRef = useRef({ value: props.value, label: initialLabel })
  const custom = props.customOption?.(inputValue) ?? null
  const { selected, items } = resolveOptions(props.options, props.value, custom)
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
      items={items}
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
      <ComboboxInput
        ref={inputRef}
        id={props.id}
        placeholder={props.placeholder}
        disabled={props.disabled}
        className="w-full"
      />
      <ComboboxContent>
        <ComboboxEmpty>{emptyText}</ComboboxEmpty>
        <ComboboxList>
          {(option: PickerOption) => (
            <ComboboxItem key={option.value} value={option}>
              <span className="min-w-0 flex-1 truncate">{option.label}</span>
              {option.hint === null ? null : (
                <span className="ml-auto max-w-[45%] shrink-0 truncate pl-2 text-xs text-muted-foreground">
                  {option.hint}
                </span>
              )}
            </ComboboxItem>
          )}
        </ComboboxList>
      </ComboboxContent>
    </Combobox>
  )
}
