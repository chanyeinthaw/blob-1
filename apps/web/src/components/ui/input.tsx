import * as React from 'react'
import { Input as InputPrimitive } from '@base-ui/react/input'
import { cn } from '~/lib/utils'

function Input({ className, type, ...props }: React.ComponentProps<'input'>) {
  return (
    <InputPrimitive
      type={type}
      data-slot="input"
      className={cn(
        'h-10 w-full min-w-0 rounded-full border border-black/[0.07] bg-secondary px-3.5 py-1 text-base transition-colors outline-none file:inline-flex file:h-6 file:border-0 file:bg-transparent file:text-sm file:font-medium file:text-foreground placeholder:text-muted-foreground hover:bg-border/60 focus-visible:border-foreground/35 focus-visible:bg-background focus-visible:ring-0 disabled:pointer-events-none disabled:cursor-not-allowed disabled:opacity-50 aria-invalid:border-destructive md:h-8 md:text-sm dark:border-white/10 dark:bg-input/40 dark:aria-invalid:border-destructive/50',
        className,
      )}
      {...props}
    />
  )
}

export { Input }
