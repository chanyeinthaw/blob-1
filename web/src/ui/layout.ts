import { clsx } from 'clsx'
import { Html, HtmlBuilder } from 'foldkit/html'

import * as Icon from './icon'

const panelClassName =
  'flex min-h-0 flex-1 bg-fog md:relative md:z-10 md:overflow-hidden md:rounded-l-2xl md:shadow-[0_1px_4px_rgb(0_0_0/0.06),0_8px_28px_rgb(0_0_0/0.10)]'

const scrollClassName =
  'min-w-0 flex-1 px-5 pb-10 md:min-h-0 md:overflow-y-auto md:pt-4 md:[mask-image:linear-gradient(to_bottom,transparent,black_16px,black_calc(100%-24px),transparent)]'

type PageConfig = Readonly<{
  title: string
  actions?: ReadonlyArray<Html>
  wideActions?: boolean
  wide?: boolean
  children: ReadonlyArray<Html>
}>

export const page = <Message>(
  config: PageConfig,
  h: HtmlBuilder<Message>,
): Html =>
  h.div(
    [h.Class(clsx(panelClassName, 'flex-col'))],
    [
      h.div(
        [
          h.Class(
            'flex shrink-0 flex-wrap items-center gap-x-4 gap-y-3 px-5 pb-4 pt-6 md:px-10 md:py-4 lg:px-16',
          ),
        ],
        [
          h.h1(
            [h.Class('mr-auto text-2xl font-semibold text-ink')],
            [config.title],
          ),
          h.div(
            [
              h.Class(
                clsx(
                  'flex flex-wrap items-center gap-2',
                  config.wideActions === true
                    ? 'w-full md:ml-auto md:w-auto'
                    : 'ml-auto',
                ),
              ),
            ],
            [...(config.actions ?? [])],
          ),
        ],
      ),
      h.div(
        [h.Class(clsx(scrollClassName, 'md:px-10 lg:px-16'))],
        [
          h.div(
            [h.Class(clsx('w-full', config.wide !== true && 'max-w-[720px]'))],
            [...config.children],
          ),
        ],
      ),
    ],
  )

export const section = <Message>(
  title: string,
  children: ReadonlyArray<Html>,
  h: HtmlBuilder<Message>,
): Html =>
  h.section(
    [h.Class('mt-8 first:mt-0')],
    [
      h.h2([h.Class('mb-2 text-body font-medium text-ink')], [title]),
      ...children,
    ],
  )

export const rowList = <Message>(
  rows: ReadonlyArray<Html>,
  h: HtmlBuilder<Message>,
): Html => h.ul([h.Class('flex flex-col divide-y divide-hair')], [...rows])

export const row = <Message>(
  children: ReadonlyArray<Html>,
  h: HtmlBuilder<Message>,
): Html =>
  h.li([h.Class('flex min-h-10 items-center gap-4 py-1')], [...children])

export const emptyLine = <Message>(
  text: string,
  h: HtmlBuilder<Message>,
): Html => h.p([h.Class('py-2 text-body text-slate')], [text])

type SplitPageConfig = Readonly<{
  title: string
  items: ReadonlyArray<Html>
  isListView: boolean
  detail: Readonly<{
    title: string
    backHref: string
    actions?: ReadonlyArray<Html>
    children: ReadonlyArray<Html>
  }>
}>

/** A page with its own item list beside the detail. On mobile the list and the detail are separate screens, picked by `isListView`. */
export const splitPage = <Message>(
  config: SplitPageConfig,
  h: HtmlBuilder<Message>,
): Html =>
  h.div(
    [h.Class(clsx(panelClassName, 'flex-row'))],
    [
      h.aside(
        [
          h.Class(
            clsx(
              'min-w-0 flex-col md:flex md:w-64 md:flex-none md:border-r md:border-hair md:bg-shelf',
              config.isListView ? 'flex flex-1' : 'hidden',
            ),
          ),
        ],
        [
          h.div(
            [
              h.Class(
                'flex shrink-0 items-center px-5 pb-4 pt-6 md:h-16 md:p-0 md:px-5',
              ),
            ],
            [
              h.h1(
                [h.Class('text-2xl font-semibold text-ink md:text-lg')],
                [config.title],
              ),
            ],
          ),
          h.nav(
            [
              h.Class('flex flex-col gap-0.5 px-2 pb-6 md:px-3'),
              h.AriaLabel(config.title),
            ],
            [...config.items],
          ),
        ],
      ),
      h.section(
        [
          h.Class(
            clsx(
              'min-w-0 flex-1 flex-col',
              config.isListView ? 'hidden md:flex' : 'flex',
            ),
          ),
        ],
        [
          h.div(
            [
              h.Class(
                'flex shrink-0 flex-wrap items-center gap-x-3 gap-y-3 px-5 pb-4 pt-6 md:h-16 md:p-0 md:px-8',
              ),
            ],
            [
              h.a(
                [
                  h.Href(config.detail.backHref),
                  h.AriaLabel(`Back to ${config.title}`),
                  h.Class(
                    '-ml-2 grid size-8 place-items-center rounded-full text-mute hover:bg-hover hover:text-ink md:hidden',
                  ),
                ],
                [Icon.chevronLeft('size-4')],
              ),
              h.h2(
                [
                  h.Class(
                    'mr-auto text-2xl font-semibold text-ink md:text-base',
                  ),
                ],
                [config.detail.title],
              ),
              h.div(
                [h.Class('flex flex-wrap items-center gap-2')],
                [...(config.detail.actions ?? [])],
              ),
            ],
          ),
          h.div(
            [h.Class(clsx(scrollClassName, 'md:px-8'))],
            [
              h.div(
                [h.Class('w-full max-w-[720px]')],
                [...config.detail.children],
              ),
            ],
          ),
        ],
      ),
    ],
  )

type SplitItemConfig = Readonly<{
  href: string
  label: string
  icon: Html
  isActive: boolean
  trailing?: Html
}>

export const splitItem = <Message>(
  config: SplitItemConfig,
  h: HtmlBuilder<Message>,
): Html =>
  h.a(
    [
      h.Href(config.href),
      ...(config.isActive ? [h.AriaCurrent('page')] : []),
      h.Class(
        clsx(
          'flex h-12 items-center gap-3 rounded-xl px-3 text-body text-ink focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring md:h-9',
          config.isActive ? 'md:bg-hair/60' : 'hover:bg-hover',
        ),
      ),
    ],
    [
      config.icon,
      h.span([h.Class('min-w-0 flex-1 truncate')], [config.label]),
      ...(config.trailing === undefined ? [] : [config.trailing]),
      Icon.chevronRight('size-4 shrink-0 text-mute md:hidden'),
    ],
  )
