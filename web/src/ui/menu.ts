import { clsx } from 'clsx'
import { Array, Option } from 'effect'
import { type Html, type HtmlBuilder, childAttributes } from 'foldkit/html'

import { Listbox } from '@foldkit/ui'

import * as Icon from './icon'

export const Menu = Listbox.create<string>()

export type MenuItem = Readonly<{
  value: string
  label: string
  hint?: string
  tone?: 'danger'
  isDisabled?: boolean
}>

type ViewConfig<Message> = Readonly<{
  model: Listbox.Model
  items: ReadonlyArray<MenuItem>
  selected?: string
  label: string
  triggerContent: Html
  triggerClassName: string
  panelClassName: string
  placement?: 'bottom-start' | 'bottom-end'
  isDisabled?: boolean
  className?: string
  toParentMessage: (message: Listbox.Message) => Message
}>

const itemClassName =
  'group flex min-h-8 cursor-pointer items-center gap-2 rounded-sm px-2 py-1 text-body text-ink data-[active]:bg-hover data-[disabled]:cursor-not-allowed data-[disabled]:opacity-50'

export const init = (id: string): Listbox.Model => Listbox.init({ id })

export const view = <Message>(
  config: ViewConfig<Message>,
  h: HtmlBuilder<Message>,
): Html => {
  const { model, items, toParentMessage } = config
  const find = (value: string): Option.Option<MenuItem> =>
    Array.findFirst(items, item => item.value === value)

  return h.submodel({
    slotId: model.id,
    model,
    view: Menu.view,
    viewInputs: {
      anchor: {
        placement: config.placement ?? 'bottom-start',
        gap: 4,
        padding: 8,
      },
      isDisabled: config.isDisabled === true,
      items: Array.map(items, item => item.value),
      isItemDisabled: value =>
        Option.exists(find(value), item => item.isDisabled === true),
      maybeSelectedValue: Option.fromNullishOr(config.selected),
      itemToConfig: value => {
        const item = find(value)
        return {
          className: itemClassName,
          content: h.div(
            [h.Class('flex min-w-0 flex-1 items-center gap-2')],
            [
              Icon.check(
                'size-3.5 shrink-0 text-ink invisible group-data-[selected]:visible',
              ),
              h.span(
                [
                  h.Class(
                    clsx(
                      'truncate',
                      Option.exists(item, i => i.tone === 'danger') &&
                        'text-alarm',
                    ),
                  ),
                ],
                [
                  Option.match(item, {
                    onNone: () => value,
                    onSome: i => i.label,
                  }),
                ],
              ),
              h.span(
                [h.Class('ml-auto shrink-0 pl-3 text-caption text-mute')],
                [
                  Option.getOrElse(
                    Option.flatMapNullishOr(item, i => i.hint),
                    () => '',
                  ),
                ],
              ),
            ],
          ),
        }
      },
      buttonContent: config.triggerContent,
      buttonAttributes: childAttributes([
        h.Class(config.triggerClassName),
        h.AriaLabel(config.label),
      ]),
      itemsAttributes: childAttributes([
        h.Class(
          clsx(
            'z-30 overflow-hidden rounded-md border border-hair bg-white p-1 shadow-md outline-none',
            config.panelClassName,
          ),
        ),
      ]),
      backdropAttributes: childAttributes([h.Class('fixed inset-0 z-20')]),
      attributes: childAttributes([
        h.Class(clsx('relative', model.isOpen && 'z-[21]', config.className)),
      ]),
    },
    toParentMessage,
  })
}
