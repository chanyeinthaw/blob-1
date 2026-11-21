import { clsx } from 'clsx'
import { Array, Option } from 'effect'
import { type Html, type HtmlBuilder, childAttributes } from 'foldkit/html'

import { Listbox } from '@foldkit/ui'

import * as Icon from './icon'

export const Dropdown = Listbox.create<string>()

export type DropdownOption = Readonly<{
  value: string
  label: string
}>

type ViewConfig<Message> = Readonly<{
  model: Listbox.Model
  options: ReadonlyArray<DropdownOption>
  selected: string
  toParentMessage: (message: Listbox.Message) => Message
  isMono?: boolean
  isDisabled?: boolean
  className?: string
  label?: string
}>

const triggerClassName =
  'flex h-8 w-full cursor-pointer items-center justify-between gap-2 rounded-full border border-control bg-white px-3.5 text-left shadow-xs text-body text-ink select-none hover:bg-hover focus-visible:border-ring focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50 data-[disabled]:cursor-not-allowed data-[disabled]:bg-chip data-[disabled]:text-mute'

const itemsClassName =
  'z-30 w-[var(--button-width,12rem)] max-w-[calc(100vw-16px)] overflow-hidden rounded-xl border border-hair bg-white p-1 shadow-md outline-none'

const itemClassName =
  'group flex h-7 cursor-pointer items-center gap-2 rounded-lg px-2 text-body text-ink data-[active]:bg-hover'

export const view = <Message>(
  config: ViewConfig<Message>,
  h: HtmlBuilder<Message>,
): Html => {
  const { model, options, selected, toParentMessage } = config
  const labelOf = (value: string): string =>
    Option.match(
      Array.findFirst(options, option => option.value === value),
      { onNone: () => value, onSome: option => option.label },
    )
  const textClassName = config.isMono === true ? 'font-mono' : undefined

  return h.submodel({
    slotId: model.id,
    model,
    view: Dropdown.view,
    viewInputs: {
      anchor: { placement: 'bottom-start', gap: 4, padding: 8 },
      isDisabled: config.isDisabled === true,
      items: Array.map(options, option => option.value),
      maybeSelectedValue: Option.some(selected),
      itemToConfig: value => ({
        className: itemClassName,
        content: h.div(
          [h.Class('flex min-w-0 flex-1 items-center gap-2')],
          [
            h.span(
              [h.Class(clsx('truncate', textClassName))],
              [labelOf(value)],
            ),
            Icon.check(
              'ml-auto size-3.5 shrink-0 text-ink invisible group-data-[selected]:visible',
            ),
          ],
        ),
      }),
      buttonContent: h.span(
        [h.Class('flex min-w-0 flex-1 items-center justify-between gap-2')],
        [
          h.span(
            [h.Class(clsx('truncate', textClassName))],
            [labelOf(selected)],
          ),
          Icon.chevronDown('size-3.5 shrink-0 text-mute'),
        ],
      ),
      buttonAttributes: childAttributes([
        h.Class(triggerClassName),
        h.AriaLabel(
          `${config.label ?? 'Choose an option'}: ${labelOf(selected)}`,
        ),
      ]),
      itemsAttributes: childAttributes([h.Class(itemsClassName)]),
      backdropAttributes: childAttributes([h.Class('fixed inset-0 z-20')]),
      attributes: childAttributes([
        h.Class(
          clsx(
            'relative',
            model.isOpen && 'z-[21]',
            config.className ?? 'w-full',
          ),
        ),
      ]),
    },
    toParentMessage,
  })
}

export const init = (id: string): Listbox.Model => Listbox.init({ id })

export const buttonId = Listbox.buttonId
