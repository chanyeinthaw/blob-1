import { clsx } from 'clsx'
import { Option } from 'effect'
import { Html, HtmlBuilder } from 'foldkit/html'

import { Button as UiButton } from '@foldkit/ui'

export type Variant =
  | 'primary'
  | 'outline'
  | 'secondary'
  | 'destructive'
  | 'danger'

const interactionClassName =
  'cursor-pointer rounded-md text-body font-medium focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring data-[disabled]:cursor-not-allowed'

const variantClassNames: Record<Variant, string> = {
  primary:
    'inline-flex h-8 items-center justify-center bg-accent px-3 text-white shadow-xs hover:bg-accent/90 data-[disabled]:opacity-50',
  outline:
    'inline-flex h-8 items-center justify-center border border-control bg-white px-3 text-ink shadow-xs hover:bg-hover data-[disabled]:opacity-50',
  secondary:
    'inline-flex h-8 items-center px-3 text-ink hover:bg-hover data-[disabled]:opacity-50',
  destructive:
    '-mr-2 inline-flex h-8 items-center px-2 text-alarm hover:bg-alarm/8 data-[disabled]:opacity-50',
  danger:
    'inline-flex h-8 items-center justify-center bg-alarm px-3 text-white shadow-xs hover:bg-alarm/90',
}

export const variantClassName = (variant: Variant): string =>
  clsx(interactionClassName, variantClassNames[variant])

type ViewConfig<Message> = Readonly<{
  label: string
  variant: Variant
  maybeOnClick?: Option.Option<Message>
  type?: 'button' | 'submit'
  isDisabled?: boolean
  isFullWidth?: boolean
  icon?: Html
}>

export const view = <Message>(
  config: ViewConfig<Message>,
  h: HtmlBuilder<Message>,
): Html => {
  const {
    label,
    variant,
    maybeOnClick = Option.none(),
    type = 'button',
    isDisabled = false,
    isFullWidth = false,
    icon,
  } = config

  return UiButton.view(
    {
      type,
      isDisabled,
      toView: attributes =>
        h.button(
          [
            ...attributes.button,
            h.Class(
              clsx(
                variantClassName(variant),
                icon !== undefined && 'gap-1.5 pl-2.5',
                isFullWidth && 'w-full',
              ),
            ),
          ],
          [...(icon === undefined ? [] : [icon]), label],
        ),
      ...Option.match(maybeOnClick, {
        onNone: () => ({}),
        onSome: onClick => ({ onClick }),
      }),
    },
    h,
  )
}

type IconViewConfig<Message> = Readonly<{
  label: string
  icon: Html
  onClick: Message
  tone?: 'default' | 'danger'
  isOutlined?: boolean
  isDisabled?: boolean
  type?: 'button' | 'submit'
}>

export const iconView = <Message>(
  config: IconViewConfig<Message>,
  h: HtmlBuilder<Message>,
): Html =>
  UiButton.view(
    {
      type: config.type ?? 'button',
      isDisabled: config.isDisabled ?? false,
      onClick: config.onClick,
      toView: attributes =>
        h.button(
          [
            ...attributes.button,
            h.AriaLabel(config.label),
            h.Title(config.label),
            h.Class(
              clsx(
                'inline-flex size-8 shrink-0 cursor-pointer items-center justify-center rounded-md focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring data-[disabled]:cursor-not-allowed data-[disabled]:opacity-40',
                config.isOutlined === true
                  ? 'border border-control bg-white text-ink shadow-xs'
                  : 'text-mute',
                config.tone === 'danger'
                  ? 'hover:bg-alarm/10 hover:text-alarm'
                  : 'hover:bg-hover hover:text-ink',
              ),
            ),
          ],
          [config.icon],
        ),
    },
    h,
  )
