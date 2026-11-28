import type { ApiError } from '@blob-1/client/api'
import { Cause, Option } from 'effect'
import type { ReactNode } from 'react'

import { Button } from '~/components/ui/button'
import { cn } from '~/lib/utils'

export const focusRingClassName =
  'outline-none focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring/40'

export const panelClassName =
  'flex min-h-0 flex-1 bg-background md:relative md:z-10 md:overflow-hidden md:rounded-l-2xl md:shadow-[0_1px_4px_rgb(0_0_0/0.06),0_8px_28px_rgb(0_0_0/0.10)]'

export const scrollClassName =
  'min-w-0 flex-1 px-5 pb-[max(2.5rem,env(safe-area-inset-bottom))] md:flex md:min-h-0 md:flex-col md:overflow-y-auto md:pt-4 md:pb-10 md:px-8'

/** Content column widths: forms by default, `wide` for lists, `full` for dense lists. */
export type ColumnWidth = 'form' | 'wide' | 'full'

export const columnClassName = {
  form: 'max-w-[720px]',
  wide: 'max-w-[1120px]',
  full: 'max-w-none',
} satisfies Record<ColumnWidth, string>

/** Shared hover affordance for catalog-table rows, matching `SplitItem`'s hover. */
export const tableRowHoverClassName = 'transition-colors hover:bg-border/40'

interface PageProps {
  readonly title: string
  readonly actions?: ReactNode
  /** Puts the actions on their own full-width row on mobile. */
  readonly wideActions?: boolean
  readonly width?: ColumnWidth
  readonly children: ReactNode
}

/** The content panel: a fixed title row with actions above a scrolling body. */
export function Page(props: PageProps) {
  const columnWidth = columnClassName[props.width ?? 'form']
  return (
    <div className={cn(panelClassName, 'flex-col')}>
      <div className="shrink-0 px-5 pt-5 pb-4 md:px-8 md:py-4">
        <div className="flex w-full flex-wrap items-center gap-x-4 gap-y-3">
          <h1 className="mr-auto text-lg font-semibold tracking-tight">{props.title}</h1>
          {props.actions === undefined || props.actions === null ? null : (
            <div
              className={cn(
                'flex flex-wrap items-center gap-2',
                props.wideActions === true ? 'w-full md:ml-auto md:w-auto' : 'ml-auto',
              )}
            >
              {props.actions}
            </div>
          )}
        </div>
      </div>
      <div className={cn(scrollClassName)}>
        <div className={cn('flex w-full flex-col md:flex-1', columnWidth)}>{props.children}</div>
      </div>
    </div>
  )
}

interface SectionProps {
  readonly title: string
  /** Rendered right after the title, e.g. a help tooltip. */
  readonly titleExtra?: ReactNode
  readonly action?: ReactNode
  readonly className?: string
  readonly children: ReactNode
}

export function Section(props: SectionProps) {
  return (
    <section className={cn('mt-8 first:mt-0', props.className)}>
      <div className="mb-2.5 flex items-center justify-between gap-2">
        <h2 className="flex items-center gap-1.5 text-sm font-semibold text-foreground">
          {props.title}
          {props.titleExtra}
        </h2>
        {props.action}
      </div>
      {props.children}
    </section>
  )
}

type FieldRowProps = (
  | { readonly id: string; readonly labelId?: undefined }
  | { readonly id?: undefined; readonly labelId: string }
) & {
  readonly label: string
  /** Rendered right after the label, e.g. a help tooltip. Kept out of the
   *  labelled element so `aria-labelledby` stays clean. */
  readonly labelExtra?: ReactNode
  readonly children: ReactNode
}

/** A label beside its control on desktop, above it on mobile. */
export function FieldRow(props: FieldRowProps) {
  return (
    <div className="grid gap-1.5 py-2 md:grid-cols-[104px_minmax(0,1fr)] md:items-start md:gap-4 md:py-1.5">
      <div className="flex min-w-0 items-center gap-1.5 md:pt-1.5">
        {props.id !== undefined ? (
          <label htmlFor={props.id} className="text-sm font-medium text-muted-foreground">
            {props.label}
          </label>
        ) : (
          <span id={props.labelId} className="text-sm font-medium text-muted-foreground">
            {props.label}
          </span>
        )}
        {props.labelExtra}
      </div>
      <div className="min-w-0">{props.children}</div>
    </div>
  )
}

interface FieldErrorProps {
  readonly id?: string
  readonly children: ReactNode
}

export function FieldError(props: FieldErrorProps) {
  return (
    <p id={props.id} role="alert" className="mt-1.5 px-3.5 text-xs text-destructive">
      {props.children}
    </p>
  )
}

interface EmptyLineProps {
  readonly children: ReactNode
}

export function EmptyLine(props: EmptyLineProps) {
  return <p className="py-2 text-sm text-muted-foreground">{props.children}</p>
}

interface EmptyProps {
  readonly children: ReactNode
}

/** A page-level empty state, centered in the remaining space of the panel. */
export function Empty(props: EmptyProps) {
  return (
    <div className="flex flex-1 items-center justify-center py-12">
      <p className="text-sm text-muted-foreground">{props.children}</p>
    </div>
  )
}

export function pageErrorMessage(cause: Cause.Cause<ApiError>): string {
  return Option.match(Cause.findErrorOption(cause), {
    onNone: () => 'Something went wrong. Try again.',
    onSome: error => error.message,
  })
}

interface PageErrorProps {
  readonly cause: Cause.Cause<ApiError>
  readonly onRetry: () => void
}

/** Shows a concise load failure message with a recovery action. */
export function PageError(props: PageErrorProps) {
  return (
    <div className="flex flex-col items-start gap-3 py-2">
      <p role="alert" className="break-words text-sm text-destructive [overflow-wrap:anywhere]">
        {pageErrorMessage(props.cause)}
      </p>
      <Button type="button" variant="outline" onClick={props.onRetry}>
        Try again
      </Button>
    </div>
  )
}
