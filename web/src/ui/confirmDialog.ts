import { Html, HtmlBuilder } from 'foldkit/html'

import { Dialog } from '@foldkit/ui'

import { variantClassName } from './button'

type ViewConfig<Message> = Readonly<{
  model: Dialog.Model
  title: string
  maybeDetail?: string
  confirmLabel: string
  onConfirm: Message
  toParentMessage: (message: Dialog.Message) => Message
}>

const dialogClassName =
  'bg-transparent p-0 open:flex items-center justify-center'

const panelClassName =
  'relative m-auto w-[min(420px,calc(100vw-2rem))] rounded-2xl border border-hair bg-white p-6 shadow-lg'

export const view = <Message>(
  config: ViewConfig<Message>,
  h: HtmlBuilder<Message>,
): Html => {
  const { model, title, confirmLabel, onConfirm, toParentMessage } = config
  const detail = config.maybeDetail

  return h.submodel({
    slotId: model.id,
    model,
    view: Dialog.view,
    viewInputs: {
      hasDescription: detail !== undefined,
      toView: render =>
        h.dialog(
          [...render.dialog, h.Class(dialogClassName)],
          render.isVisible
            ? [
                h.div([
                  ...render.backdrop,
                  h.Class('fixed inset-0 bg-black/50'),
                ]),
                h.div(
                  [...render.panel, h.Class(panelClassName)],
                  [
                    h.h2(
                      [
                        ...render.title,
                        h.Class(
                          'text-subheading font-semibold leading-none text-ink',
                        ),
                      ],
                      [title],
                    ),
                    ...(detail === undefined
                      ? []
                      : [
                          h.p(
                            [
                              ...render.description,
                              h.Class('mt-2 break-all text-body text-mute'),
                            ],
                            [detail],
                          ),
                        ]),
                    h.div(
                      [h.Class('mt-6 flex items-center justify-end gap-2')],
                      [
                        h.button(
                          [
                            ...render.closeButton,
                            h.Class(variantClassName('outline')),
                          ],
                          ['Cancel'],
                        ),
                        h.button(
                          [
                            h.Type('button'),
                            h.OnClick(onConfirm),
                            h.Class(variantClassName('danger')),
                          ],
                          [confirmLabel],
                        ),
                      ],
                    ),
                  ],
                ),
              ]
            : [],
        ),
    },
    toParentMessage,
  })
}
