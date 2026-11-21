import { Html, HtmlBuilder } from 'foldkit/html'

type PageConfig = Readonly<{
  title: string
  actions?: ReadonlyArray<Html>
  wideActions?: boolean
  children: ReadonlyArray<Html>
}>

export const page = <Message>(
  config: PageConfig,
  h: HtmlBuilder<Message>,
): Html =>
  h.div(
    [
      h.Class(
        'flex min-h-[calc(100dvh-4rem)] flex-col bg-fog md:h-full md:min-h-0 md:overflow-hidden md:rounded-xl md:border md:border-hair',
      ),
    ],
    [
      h.div(
        [
          h.Class(
            'sticky top-0 z-10 flex min-h-14 shrink-0 flex-wrap items-center gap-x-2 gap-y-2 border-b border-hair bg-fog py-2.5 pl-5 pr-3 md:static',
          ),
        ],
        [
          h.h1(
            [h.Class('text-subheading font-medium text-ink')],
            [config.title],
          ),
          h.div(
            [
              h.Class(
                config.wideActions === true
                  ? 'flex w-full items-center gap-2 md:ml-auto md:w-auto'
                  : 'ml-auto flex items-center gap-2',
              ),
            ],
            [...(config.actions ?? [])],
          ),
        ],
      ),
      h.div(
        [
          h.Class(
            'min-w-0 flex-1 px-5 py-6 md:overflow-y-auto md:px-5 md:py-6',
          ),
        ],
        [h.div([h.Class('w-full max-w-[720px]')], [...config.children])],
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
