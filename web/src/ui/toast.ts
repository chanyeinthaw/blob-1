import { clsx } from 'clsx'
import { Schema } from 'effect'
import { Html, HtmlBuilder } from 'foldkit/html'

import { Toast as UiToast } from '@foldkit/ui'

const toast = UiToast.make(
  Schema.Struct({
    title: Schema.String,
  }),
)

export const Model = toast.Model
export const Message = toast.Message
export const OutMessage = toast.OutMessage
export const init = toast.init
export const update = toast.update
export const show = toast.show

export const ToastVariant = UiToast.Variant
export type ToastVariant = typeof ToastVariant.Type

type Entry = typeof toast.Entry.Type

const entryView = <ParentMessage>(
  entry: Entry,
  h: HtmlBuilder<ParentMessage>,
): Html =>
  h.p(
    [
      h.Class(
        clsx(
          'w-[min(356px,calc(100vw-2rem))] rounded-full border border-hair bg-white px-4 py-3 text-body font-medium text-ink shadow-lg motion-safe:transition-opacity motion-safe:data-[closed]:opacity-0',
          entry.variant === 'Error' && 'text-alarm',
        ),
      ),
    ],
    [entry.payload.title],
  )

export const view = <ParentMessage>(
  model: typeof Model.Type,
  toParentMessage: (message: typeof Message.Type) => ParentMessage,
  h: HtmlBuilder<ParentMessage>,
): Html =>
  h.submodel({
    slotId: model.id,
    model,
    view: toast.view,
    viewInputs: {
      position: 'BottomCenter',
      entryToView: (entry, _handlers) => entryView(entry, h),
    },
    toParentMessage,
  })
