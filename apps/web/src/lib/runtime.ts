import { apiLayer } from '@blob-1/client/api'
import { mockTransport } from '@blob-1/client/mock'
import { sessionLive, sessionMock } from '@blob-1/client/session'
import { wsTransport } from '@blob-1/client/ws'
import { AtomMachine } from '@typeonce/effect-machine/reactivity'
import { Layer } from 'effect'
import { Atom } from 'effect/reactivity'

import { notifyLive } from './notify'

const isMock = import.meta.env.VITE_MOCK === '1'

export const sessionLayer = isMock ? sessionMock : sessionLive

/** Services every page can use: the API over one shared socket, the session, and toasts. */
export const AppLayer = Layer.mergeAll(
  apiLayer.pipe(Layer.provide(isMock ? mockTransport : wsTransport)),
  sessionLayer,
  notifyLive,
)

export const AppRuntime = Atom.runtime(AppLayer)

export const MachineAtoms = AtomMachine.bind(AppRuntime)
