import * as React from 'react'
import { cn } from '~/lib/utils'

function Textarea({ className, ...props }: React.ComponentProps<'textarea'>) {
  return (
    <textarea
      data-slot="textarea"
      className={cn(
        'flex field-sizing-content min-h-16 w-full rounded-2xl border border-black/[0.07] bg-secondary px-3.5 py-2.5 text-base transition-colors outline-none placeholder:text-muted-foreground hover:bg-border/60 focus-visible:border-foreground/35 focus-visible:bg-background disabled:cursor-not-allowed disabled:opacity-50 aria-invalid:border-destructive md:text-sm dark:border-white/10 dark:bg-input/40 dark:aria-invalid:border-destructive/50',
        className,
      )}
      {...props}
    />
  )
}

export { Textarea }
