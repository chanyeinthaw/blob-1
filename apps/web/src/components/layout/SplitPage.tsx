import { Link, type LinkProps } from '@tanstack/react-router'
import { ChevronLeft, ChevronRight } from 'lucide-react'
import { type ReactNode, useEffect, useRef } from 'react'

import { cn } from '~/lib/utils'

import {
  type ColumnWidth,
  columnClassName,
  focusRingClassName,
  panelClassName,
  scrollClassName,
} from './Page'

interface SplitPageProps {
  readonly title: string
  /** Rendered before the title, e.g. a back button. */
  readonly titleLeading?: ReactNode
  readonly titleExtra?: ReactNode
  readonly items: ReactNode
  /** On mobile the list and the detail are separate screens; this picks the list. */
  readonly isListView: boolean
  readonly children: ReactNode
}

/** A page with its own item list beside the selected item's detail (a `SplitDetail`). */
export function SplitPage(props: SplitPageProps) {
  const asideRef = useRef<HTMLElement>(null)
  const prevIsListView = useRef(props.isListView)
  useEffect(() => {
    if (!prevIsListView.current && props.isListView) {
      const first = asideRef.current?.querySelector('a')
      if (first instanceof HTMLElement) first.focus()
    }
    prevIsListView.current = props.isListView
  }, [props.isListView])

  return (
    <div data-list-view={props.isListView} className={cn(panelClassName, 'group/split flex-row')}>
      <aside
        ref={asideRef}
        className={cn(
          'min-w-0 flex-col md:flex md:w-56 md:flex-none md:border-r md:bg-shelf',
          props.isListView ? 'flex flex-1' : 'hidden',
        )}
      >
        <div className="flex shrink-0 items-center gap-2 px-5 pt-5 pb-4 md:min-h-16 md:px-5 md:py-4">
          {props.titleLeading}
          <h1 className="mr-auto truncate text-2xl font-semibold tracking-tight md:text-lg">
            {props.title}
          </h1>
          {props.titleExtra}
        </div>
        <nav aria-label={props.title} className="flex flex-col gap-1 px-2 pb-6 md:px-3">
          {props.items}
        </nav>
      </aside>
      {props.children}
    </div>
  )
}

interface SplitGroupLabelProps {
  readonly children: ReactNode
}

/** Subtle section header inside a `SplitPage` sidebar. */
export function SplitGroupLabel(props: SplitGroupLabelProps) {
  return (
    <p className="mt-4 mb-1 px-3 text-[11px] font-medium tracking-wider text-muted-foreground/80 uppercase">
      {props.children}
    </p>
  )
}

interface SplitDetailProps {
  readonly title: string
  readonly isListView: boolean
  readonly backTo: NonNullable<LinkProps['to']>
  readonly backParams?: LinkProps['params']
  readonly backLabel: string
  readonly actions?: ReactNode
  readonly width?: ColumnWidth
  readonly children: ReactNode
}

export function SplitDetail(props: SplitDetailProps) {
  const headingRef = useRef<HTMLHeadingElement>(null)
  const prevIsListView = useRef(props.isListView)
  useEffect(() => {
    if (prevIsListView.current && !props.isListView) {
      headingRef.current?.focus()
    }
    prevIsListView.current = props.isListView
  }, [props.isListView])

  const columnWidth = columnClassName[props.width ?? 'form']

  return (
    <section className="flex min-w-0 flex-1 flex-col group-data-[list-view=true]/split:hidden md:group-data-[list-view=true]/split:flex">
      <div className="shrink-0 px-5 pt-5 pb-4 md:px-8 md:py-4">
        <div className="flex w-full flex-wrap items-center gap-x-1.5 gap-y-2 md:min-h-8 md:gap-x-3">
          <Link
            to={props.backTo}
            {...(props.backParams !== undefined ? { params: props.backParams } : {})}
            aria-label={`Back to ${props.backLabel}`}
            className={cn(
              '-ml-2.5 grid size-10 shrink-0 place-items-center rounded-full text-muted-foreground transition-colors hover:bg-accent hover:text-foreground md:hidden',
              focusRingClassName,
            )}
          >
            <ChevronLeft className="size-4" />
          </Link>
          {props.isListView ? (
            <h2
              ref={headingRef}
              tabIndex={-1}
              className="mr-auto text-lg font-semibold tracking-tight outline-none"
            >
              {props.title}
            </h2>
          ) : (
            <h1
              ref={headingRef}
              tabIndex={-1}
              className="mr-auto text-lg font-semibold tracking-tight outline-none"
            >
              {props.title}
            </h1>
          )}
          {props.actions === undefined || props.actions === null ? null : (
            <div className="flex min-w-0 max-w-full flex-wrap items-center gap-2">
              {props.actions}
            </div>
          )}
        </div>
      </div>
      <div className={cn(scrollClassName, 'md:px-8')}>
        <div className={cn('w-full', columnWidth)}>{props.children}</div>
      </div>
    </section>
  )
}

interface SplitItemProps {
  readonly to: NonNullable<LinkProps['to']>
  readonly params?: LinkProps['params']
  readonly label: string
  readonly icon: ReactNode
  readonly isActive: boolean
  readonly trailing?: ReactNode
}

export function SplitItem(props: SplitItemProps) {
  return (
    <Link
      to={props.to}
      {...(props.params !== undefined ? { params: props.params } : {})}
      className={cn(
        'flex h-12 items-center gap-3 rounded-xl px-3 text-sm font-medium transition-colors hover:bg-border/50 md:h-9',
        focusRingClassName,
        props.isActive && 'md:bg-background md:shadow-xs md:ring-1 md:ring-border',
      )}
    >
      {props.icon}
      <span className="min-w-0 flex-1 truncate">{props.label}</span>
      {props.trailing}
      <ChevronRight className="size-4 shrink-0 text-muted-foreground md:hidden" />
    </Link>
  )
}
