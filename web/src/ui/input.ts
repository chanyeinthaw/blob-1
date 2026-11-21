import { clsx } from 'clsx'
import { Option } from 'effect'
import { Html, HtmlBuilder } from 'foldkit/html'

import { Input as UiInput } from '@foldkit/ui'

type ViewConfig<Message> = Readonly<{
  id: string
  value: string
  onInput: (value: string) => Message
  maybeError: Option.Option<string>
  type?: 'text' | 'password' | 'email' | 'url'
  isMono?: boolean
  isAutofocus?: boolean
  isDisabled?: boolean
  placeholder?: string
  inputMode?: 'numeric'
  autoComplete?: 'current-password' | 'off'
}>

const inputClassName =
  'h-8 w-full rounded-full border bg-white shadow-xs px-3.5 text-body text-ink placeholder:text-mute focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50 data-[disabled]:opacity-60'

export const view = <Message>(
  config: ViewConfig<Message>,
  h: HtmlBuilder<Message>,
): Html => {
  const {
    id,
    value,
    onInput,
    maybeError,
    type = 'text',
    isMono = false,
    isAutofocus = false,
    isDisabled = false,
    placeholder,
    inputMode,
    autoComplete,
  } = config
  const isInvalid = Option.isSome(maybeError)

  return UiInput.view(
    {
      id,
      value,
      type,
      onInput,
      isAutofocus,
      isDisabled,
      isInvalid,
      hasDescription: isInvalid,
      toView: attributes =>
        h.div(
          [],
          [
            h.input([
              ...attributes.input,
              ...(placeholder === undefined
                ? []
                : [h.Placeholder(placeholder)]),
              ...(inputMode === undefined
                ? []
                : [h.Attribute('inputmode', inputMode)]),
              ...(autoComplete === undefined
                ? []
                : [h.Autocomplete(autoComplete)]),
              h.Class(
                clsx(
                  inputClassName,
                  isMono && 'font-mono placeholder:font-sans',
                  isInvalid
                    ? 'border-alarm'
                    : 'border-control focus-visible:border-ring',
                ),
              ),
            ]),
            ...Option.match(maybeError, {
              onNone: () => [],
              onSome: error => [
                h.p(
                  [
                    ...attributes.description,
                    h.Class('mt-1.5 text-caption text-alarm'),
                    h.Attribute('role', 'alert'),
                  ],
                  [error],
                ),
              ],
            }),
          ],
        ),
    },
    h,
  )
}
