import { Context, Effect, Layer } from 'effect'
import { toast } from 'sonner'

export interface NotifyService {
  readonly error: (message: string) => Effect.Effect<void>
}

/** Shows transient feedback for work that has no inline place to report. */
export class Notify extends Context.Service<Notify, NotifyService>()('@blob-1/web/lib/notify') {}

export const notifyLive = Layer.succeed(
  Notify,
  Notify.of({ error: message => Effect.sync(() => toast.error(message)) }),
)
