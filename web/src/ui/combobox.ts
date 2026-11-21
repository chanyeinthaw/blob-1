import { clsx } from 'clsx'
import { Option } from 'effect'
import { type Html, type HtmlBuilder, childAttributes } from 'foldkit/html'
import { modifyFields } from 'foldkit/struct'

import { Combobox as UiCombobox } from '@foldkit/ui'

import * as Icon from './icon'

export const Combobox = UiCombobox.create<string>()

export type ComboboxOption = Readonly<{
  value: string
  label: string
  maybeHint?: string
  search?: string
}>

type ViewConfig<Message> = Readonly<{
  model: UiCombobox.Model
  options: ReadonlyArray<ComboboxOption>
  selected: string
  toParentMessage: (message: UiCombobox.Message) => Message
  placeholder?: string
  isMono?: boolean
  isDisabled?: boolean
  className?: string
}>

const noActiveItem = { maybeActiveItemIndex: Option.none() }

// The textbox belongs to the combobox view; opening it uses the same message as its keyboard handling.
// oxlint-disable-next-line foldkit/no-child-message-construction-in-root
const openedMessage = UiCombobox.Message.Opened(noActiveItem)

export const init = (id: string): UiCombobox.Model => UiCombobox.init({ id })

export const setSelection = (
  model: UiCombobox.Model,
  label: string,
): UiCombobox.Model => modifyFields(model, { inputValue: () => label })

export const view = <Message>(
  config: ViewConfig<Message>,
  h: HtmlBuilder<Message>,
): Html => {
  const selected = config.options.find(
    option => option.value === config.selected,
  )
  const display = (option: ComboboxOption) =>
    config.options.filter(other => other.label === option.label).length > 1
      ? `${option.label} (${option.maybeHint || option.value})`
      : option.label
  const resting = selected === undefined ? config.selected : display(selected)
  const query =
    config.model.inputValue === resting ||
    config.model.inputValue === selected?.label
      ? ''
      : config.model.inputValue.toLowerCase()
  const options = config.options.filter(option =>
    `${option.label} ${option.maybeHint ?? ''} ${option.search ?? ''}`
      .toLowerCase()
      .includes(query),
  )
  const find = (value: string) =>
    config.options.find(option => option.value === value)
  return h.submodel({
    slotId: config.model.id,
    model: config.model.isOpen
      ? config.model
      : setSelection(config.model, resting),
    view: Combobox.view,
    viewInputs: {
      anchor: { placement: 'bottom-start', gap: 4, padding: 8 },
      isDisabled: config.isDisabled === true,
      isReadOnly: config.isDisabled === true,
      items: options.length === 0 ? [''] : options.map(option => option.value),
      isItemDisabled: value => value === '',
      maybeSelectedValue:
        config.selected === '' ? Option.none() : Option.some(config.selected),
      restingInputValue: resting,
      itemToValue: value => value,
      itemToDisplayText: value => {
        const option = find(value)
        return option === undefined ? value : display(option)
      },
      itemToConfig: (value, context) => ({
        className:
          'flex min-h-7 cursor-pointer items-center rounded-lg px-2 py-1 text-body text-ink data-[active]:bg-hover',
        content: h.div(
          [h.Class('flex min-w-0 flex-1 items-center gap-2')],
          [
            h.div(
              [h.Class('min-w-0 wrap-anywhere')],
              [
                h.span(
                  [h.Class(clsx(config.isMono && 'font-mono'))],
                  [value === '' ? 'No matches' : (find(value)?.label ?? value)],
                ),
                h.span(
                  [h.Class('ml-2 text-mute empty:hidden')],
                  [find(value)?.maybeHint ?? ''],
                ),
              ],
            ),
            Icon.check(
              context.isSelected
                ? 'ml-auto size-3.5 shrink-0 text-ink'
                : 'invisible ml-auto size-3.5 shrink-0 text-ink',
            ),
          ],
        ),
      }),
      inputAttributes: childAttributes([
        h.Class(
          clsx(
            'h-8 w-full rounded-full border border-control bg-white pl-3.5 shadow-xs pr-9 text-body text-ink placeholder:text-mute focus-visible:border-ring focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50 data-[disabled]:cursor-not-allowed data-[disabled]:bg-chip data-[disabled]:text-mute',
            config.isMono && 'font-mono placeholder:font-sans',
          ),
        ),
        ...(config.isDisabled === true || config.model.isOpen
          ? []
          : [h.OnClick(config.toParentMessage(openedMessage))]),
        h.Placeholder(config.placeholder ?? ''),
        h.AriaLabel(config.placeholder ?? 'Choose an option'),
      ]),
      inputWrapperAttributes: childAttributes([h.Class('relative')]),
      buttonContent: Icon.chevronDown('size-3.5 shrink-0 text-mute'),
      buttonAttributes: childAttributes([
        h.Class(
          'absolute inset-y-0 right-0 flex items-center px-2.5 data-[disabled]:cursor-not-allowed data-[disabled]:opacity-60',
        ),
        h.AriaLabel(config.model.isOpen ? 'Close options' : 'Open options'),
      ]),
      itemsAttributes: childAttributes([
        h.Class(
          'z-30 w-[var(--button-width,12rem)] max-w-[calc(100vw-16px)] overflow-hidden rounded-xl border border-hair bg-white p-1 shadow-md outline-none',
        ),
      ]),
      backdropAttributes: childAttributes([h.Class('fixed inset-0 z-20')]),
      attributes: childAttributes([
        h.Class(
          clsx(
            'relative',
            config.model.isOpen && 'z-[21]',
            config.className ?? 'w-full',
          ),
        ),
      ]),
    },
    toParentMessage: config.toParentMessage,
  })
}
