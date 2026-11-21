import { Html, HtmlBuilder } from 'foldkit/html'

type ViewConfig = Readonly<{
  id: string
  label: string
  control: Html
}>

export const view = <Message>(
  config: ViewConfig,
  h: HtmlBuilder<Message>,
): Html =>
  h.div(
    [
      h.Class(
        'grid gap-1.5 py-1.5 md:grid-cols-[96px_minmax(0,1fr)] md:items-start md:gap-4',
      ),
    ],
    [
      h.label(
        [
          h.For(config.id),
          h.Class('text-body font-medium text-slate md:pt-1.5'),
        ],
        [config.label],
      ),
      h.div([h.Class('min-w-0')], [config.control]),
    ],
  )
