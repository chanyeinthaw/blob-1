import type { ApiError } from '@blob-1/client/api'
import { Cause, Option } from 'effect'
import type { ReactNode } from 'react'

import { Button } from '~/components/ui/button'
import { cn } from '~/lib/utils'

export const focusRingClassName =
  'outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-inset focus-visible:ring-ring/50'

export const panelClassName =
  'flex min-h-0 flex-1 bg-background md:relative md:z-10 md:overflow-hidden md:rounded-l-2xl md:shadow-[0_1px_4px_rgb(0_0_0/0.06),0_8px_28px_rgb(0_0_0/0.10)]'

export const scrollClassName =
  'min-w-0 flex-1 px-5 pb-[max(2.5rem,env(safe-area-inset-bottom))] md:min-h-0 md:overflow-y-auto md:pt-4 md:pb-10'

interface PageProps {
  readonly title: string
  readonly actions?: ReactNode
  /** Puts the actions on their own full-width row on mobile. */
  readonly wideActions?: boolean
  /** Lets lists use the wide column instead of the 720px form column. */
  readonly wide?: boolean
  readonly children: ReactNode
}

/** The content panel: a fixed title row with actions above a scrolling body. */
export function Page(props: PageProps) {
  const columnWidth = props.wide === true ? 'max-w-[1120px]' : 'max-w-[720px]'
  return (
    <div className={cn(panelClassName, 'flex-col')}>
      <div className="shrink-0 px-5 pt-5 pb-4 md:px-10 md:py-4 lg:px-16">
        <div className={cn('flex w-full flex-wrap items-center gap-x-4 gap-y-3', columnWidth)}>
          <h1 className="mr-auto text-2xl font-semibold tracking-tight">{props.title}</h1>
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
      <div className={cn(scrollClassName, 'md:px-10 lg:px-16')}>
        <div className={cn('w-full', columnWidth)}>{props.children}</div>
      </div>
    </div>
  )
}

interface SectionProps {
  readonly title: string
  readonly className?: string
  readonly children: ReactNode
}

export function Section(props: SectionProps) {
  return (
    <section className={cn('mt-8 first:mt-0', props.className)}>
      <h2 className="mb-2.5 text-sm font-semibold text-foreground">{props.title}</h2>
      {props.children}
    </section>
  )
}

type FieldRowProps = (
  | { readonly id: string; readonly labelId?: undefined }
  | { readonly id?: undefined; readonly labelId: string }
) & {
  readonly label: string
  readonly children: ReactNode
}

/** A label beside its control on desktop, above it on mobile. */
export function FieldRow(props: FieldRowProps) {
  return (
    <div className="grid gap-1.5 py-2 md:grid-cols-[104px_minmax(0,1fr)] md:items-start md:gap-4 md:py-1.5">
      {props.id !== undefined ? (
        <label htmlFor={props.id} className="text-sm font-medium text-muted-foreground md:pt-1.5">
          {props.label}
        </label>
      ) : (
        <span id={props.labelId} className="text-sm font-medium text-muted-foreground md:pt-1.5">
          {props.label}
        </span>
      )}
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
