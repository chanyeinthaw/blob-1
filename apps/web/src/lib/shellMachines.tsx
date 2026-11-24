import { RegistryContext } from '@effect/atom-react'
import type { AtomMachine } from '@typeonce/effect-machine/reactivity'
import type { AtomRegistry } from 'effect/reactivity'
import { createContext, type ReactNode, useContext, useEffect, useState } from 'react'

type MachineRef = AtomMachine.MachineAtom.Any['ref']

class ShellScope {
  private active = false
  private readonly refs = new Set<MachineRef>()
  private readonly unmounts: Array<() => void> = []

  register(ref: MachineRef, registry: AtomRegistry.AtomRegistry) {
    if (this.refs.has(ref)) return
    this.refs.add(ref)
    if (this.active) {
      this.unmounts.push(registry.mount(ref))
    }
  }

  mountAll(registry: AtomRegistry.AtomRegistry) {
    this.active = true
    for (const ref of this.refs) {
      this.unmounts.push(registry.mount(ref))
    }
  }

  unmountAll() {
    this.active = false
    for (const unmount of this.unmounts) {
      unmount()
    }
    this.unmounts.length = 0
  }
}

const ShellScopeContext = createContext<ShellScope | null>(null)

/**
 * Keeps page machines mounted for the lifetime of the signed-in `Shell` so
 * switching settings tabs never cancels in-flight save queues or loses drafts.
 * Unmounting `Shell` (sign-out / unauthorized) disposes every mounted machine;
 * retaining `refs` on the scope makes React StrictMode remounts idempotent.
 */
export function ShellMachinesProvider(props: { readonly children: ReactNode }) {
  const registry = useContext(RegistryContext)
  const [scope] = useState(() => new ShellScope())

  useEffect(() => {
    scope.mountAll(registry)
    return () => scope.unmountAll()
  }, [registry, scope])

  return <ShellScopeContext.Provider value={scope}>{props.children}</ShellScopeContext.Provider>
}

/**
 * Like `createMachineContext`, but retains one typed machine instance per
 * `(ShellScope, scopeKey)` across route transitions so pending saves finish cleanly.
 */
export function createPersistentMachineContext<A extends AtomMachine.MachineAtom.Any, Input>(
  key: string,
  create: (options: { readonly input: Input; readonly scopeKey: string }) => A,
) {
  const Context = createContext<A | null>(null)
  const instances = new WeakMap<ShellScope, Map<string, A>>()

  function Provider(props: {
    readonly input: Input
    readonly scopeKey?: string
    readonly children: ReactNode
  }) {
    const scope = useContext(ShellScopeContext)
    const registry = useContext(RegistryContext)
    const instanceKey = props.scopeKey ?? 'default'

    const [fallback] = useState<Map<string, A>>(() => new Map())
    const bucket = (() => {
      if (scope === null) return fallback
      let map = instances.get(scope)
      if (map === undefined) {
        map = new Map<string, A>()
        instances.set(scope, map)
      }
      return map
    })()

    let machine = bucket.get(instanceKey)
    if (machine === undefined) {
      machine = create({ input: props.input, scopeKey: instanceKey })
      bucket.set(instanceKey, machine)
    }

    useEffect(() => {
      if (scope !== null) {
        scope.register(machine.ref, registry)
      }
    }, [machine, registry, scope])

    return <Context.Provider value={machine}>{props.children}</Context.Provider>
  }

  function useMachine(): A {
    const machine = useContext(Context)
    if (machine === null) {
      throw new Error(`Missing persistent machine provider for '${key}'`)
    }
    return machine
  }

  return { Provider, useMachine }
}
