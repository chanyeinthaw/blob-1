import { Layer } from 'effect'
import { Runtime } from 'foldkit'

import { apiLayer } from './api/client'
import { mockTransport } from './api/mock'
import { wsTransport } from './api/ws'
import { Flags, flags, init } from './main'
import { Message } from './message'
import { Model } from './model'
import { subscriptions } from './subscription'
import { update } from './update'
import { view } from './view'

const application = Runtime.makeApplication({
  Model,
  Flags,
  init,
  update,
  view,
  subscriptions,
  resources: apiLayer.pipe(
    Layer.provide(
      import.meta.env.VITE_MOCK === '1' ? mockTransport : wsTransport,
    ),
  ),
  container: document.getElementById('root'),
  routing: {
    onUrlRequest: request => Message.ClickedLink({ request }),
    onUrlChange: url => Message.ChangedUrl({ url }),
  },
  devTools: {
    Message,
  },
})

Runtime.run(application, { flags })
