import type { ReactElement } from 'react'

import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from '~/components/ui/alert-dialog'
import { Tooltip, TooltipContent, TooltipTrigger } from '~/components/ui/tooltip'

interface ConfirmDialogProps {
  /** The control that opens the dialog, rendered through the trigger. */
  readonly trigger: ReactElement
  readonly tooltip?: string
  readonly tooltipSide?: 'top' | 'right' | 'bottom' | 'left'
  readonly tooltipSideOffset?: number
  readonly title: string
  readonly detail?: string
  readonly confirmLabel: string
  readonly confirmVariant?: 'destructive' | 'default'
  readonly onConfirm: () => void
}

/** Asks before a destructive action; confirming closes the dialog and runs it. */
export function ConfirmDialog(props: ConfirmDialogProps) {
  return (
    <AlertDialog>
      {props.tooltip === undefined ? (
        <AlertDialogTrigger render={props.trigger} />
      ) : (
        <Tooltip>
          <TooltipTrigger render={<AlertDialogTrigger render={props.trigger} />} />
          <TooltipContent
            {...(props.tooltipSide !== undefined ? { side: props.tooltipSide } : {})}
            {...(props.tooltipSideOffset !== undefined
              ? { sideOffset: props.tooltipSideOffset }
              : {})}
          >
            {props.tooltip}
          </TooltipContent>
        </Tooltip>
      )}
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{props.title}</AlertDialogTitle>
          {props.detail === undefined ? null : (
            <AlertDialogDescription className="break-words [overflow-wrap:anywhere]">
              {props.detail}
            </AlertDialogDescription>
          )}
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <AlertDialogCancel
            variant={props.confirmVariant ?? 'destructive'}
            onClick={props.onConfirm}
          >
            {props.confirmLabel}
          </AlertDialogCancel>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}
