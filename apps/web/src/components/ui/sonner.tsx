import { Toaster as Sonner, type ToasterProps } from 'sonner'

function Toaster(props: ToasterProps) {
  return (
    <Sonner
      theme="light"
      position="bottom-center"
      offset={{ bottom: 'max(16px, env(safe-area-inset-bottom))' }}
      mobileOffset={{ bottom: 'max(16px, env(safe-area-inset-bottom))' }}
      toastOptions={{
        classNames: {
          toast:
            '!rounded-2xl !border-border !bg-popover !px-4 !py-2.5 !text-sm !text-popover-foreground !shadow-md',
        },
      }}
      {...props}
    />
  )
}

export { Toaster }
