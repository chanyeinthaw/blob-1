import { Api } from '@blob-1/client/api'
import { BotMcpState, type BotMcpServer } from '@blob-1/contracts'
import { useAtomRefresh, useAtomSet, useAtomValue } from '@effect/atom-react'
import { Link } from '@tanstack/react-router'
import { Machine } from '@typeonce/effect-machine'
import { MachineState } from '@typeonce/effect-machine-react'
import { Effect, Schema } from 'effect'
import { Plug } from 'lucide-react'
import { useEffect } from 'react'

import { EmptyLine, PageError, Section } from '~/components/layout/Page'
import { Badge } from '~/components/ui/badge'
import { Button } from '~/components/ui/button'
import { Switch } from '~/components/ui/switch'
import { Notify } from '~/lib/notify'
import { machineAtomsForBot } from '~/lib/runtime'
import { createPersistentMachineContext } from '~/lib/shellMachines'
import { matchCached } from '~/lib/useBackgroundRefresh'
import { cn } from '~/lib/utils'

import { botMcpAtomFor } from './botsState'
import { useBotInstanceKey } from './CreateBotDialog'

const fetchBotMcp = Effect.gen(function* () {
  const api = yield* Api
  return yield* api.request('bot.mcp.get', {}).pipe(Effect.orElseSucceed(() => null))
})

const toggleBotMcp = Effect.fn('BotMcp.setEnabled')(function* (args: {
  readonly id: string
  readonly enabled: boolean
}) {
  const api = yield* Api
  const notify = yield* Notify
  return yield* api
    .request('bot.mcp.setEnabled', args)
    .pipe(Effect.catch(error => notify.error(error.message).pipe(Effect.as(null))))
})

const BotMcpStates = Machine.state({
  type: 'parallel',
  fields: {
    serverData: BotMcpState,
    data: BotMcpState,
    inFlightSave: Schema.Boolean,
    mutationSeq: Schema.Finite,
  },
  states: {
    Mutate: {
      states: {
        Idle: {},
        Saving: { fields: { id: Schema.String, enabled: Schema.Boolean } },
      },
    },
    Sync: {
      states: {
        Idle: {},
        Refreshing: { fields: { mutationSeq: Schema.Finite } },
      },
    },
  },
})

const BotMcpEvents = Machine.events({
  SetEnabled: { id: Schema.String, enabled: Schema.Boolean },
  Refresh: {},
})

const BotMcpMachine = Machine.make({
  id: 'BotMcp',
  root: BotMcpStates,
  events: BotMcpEvents,
  input: BotMcpState,
  effects: { fetchBotMcp, toggleBotMcp },
}).handle({
  root: ({ input }) => ({ serverData: input, data: input, inFlightSave: false, mutationSeq: 0 }),
  states: {
    Mutate: {
      initial: { target: 'Mutate.Idle' },
      states: {
        Idle: {
          on: {
            SetEnabled: {
              target: 'Mutate.Saving',
              update: 'root',
              data: ({ root, event }) => ({
                target: { id: event.id, enabled: event.enabled },
                update: {
                  ...root,
                  data: {
                    servers: root.data.servers.map(server =>
                      server.id === event.id ? { ...server, enabledForBot: event.enabled } : server,
                    ),
                  },
                  inFlightSave: true,
                  mutationSeq: root.mutationSeq + 1,
                },
              }),
            },
          },
        },
        Saving: {
          invoke: {
            src: 'toggleBotMcp',
            input: ({ state }) => ({ id: state.id, enabled: state.enabled }),
            onDone: {
              target: 'Mutate.Idle',
              update: 'root',
              data: ({ root, output }) => ({
                target: undefined,
                update:
                  output === null
                    ? { ...root, data: root.serverData, inFlightSave: false }
                    : {
                        ...root,
                        serverData: output,
                        data: output,
                        inFlightSave: false,
                        mutationSeq: root.mutationSeq + 1,
                      },
              }),
            },
          },
        },
      },
    },
    Sync: {
      initial: { target: 'Sync.Idle' },
      states: {
        Idle: {
          on: {
            Refresh: {
              target: 'Sync.Refreshing',
              data: ({ root }) => ({ mutationSeq: root.mutationSeq }),
            },
          },
        },
        Refreshing: {
          invoke: {
            src: 'fetchBotMcp',
            onDone: {
              target: 'Sync.Idle',
              update: 'root',
              data: ({ root, state, output }) => ({
                target: undefined,
                update:
                  output !== null && state.mutationSeq === root.mutationSeq
                    ? {
                        ...root,
                        serverData: output,
                        // Never clobber an optimistic toggle while its save is still in flight.
                        data: root.inFlightSave ? root.data : output,
                      }
                    : root,
              }),
            },
          },
        },
      },
    },
  },
})

const BotMcpOwner = createPersistentMachineContext(
  'bot-mcp',
  ({ input, scopeKey }: { readonly input: BotMcpState; readonly scopeKey: string }) =>
    machineAtomsForBot(scopeKey).make(BotMcpMachine, input),
)

/** Server | Global | On, shared by the header and every row. */
const gridClassName = 'sm:grid-cols-[minmax(0,1fr)_112px_48px]'

function BotMcpHeader() {
  return (
    <div
      className={cn(
        'hidden h-10 items-center gap-x-3 border-b border-border px-2 text-sm font-medium text-muted-foreground sm:grid',
        gridClassName,
      )}
    >
      <span>Server</span>
      <span>Global</span>
      <span className="text-right">On</span>
    </div>
  )
}

interface BotMcpServerRowProps {
  readonly server: BotMcpServer
  readonly disabled: boolean
  readonly onSetEnabled: (enabled: boolean) => void
}

function BotMcpServerRow(props: BotMcpServerRowProps) {
  const { server } = props
  const switchId = `bot-mcp-enabled-${encodeURIComponent(server.id)}`
  return (
    <li
      className={cn(
        'group grid grid-cols-1 items-center gap-x-3 gap-y-2.5 px-1 py-3 sm:min-h-12 sm:gap-y-1.5 sm:px-2 sm:py-2',
        gridClassName,
      )}
    >
      <div className="flex min-w-0 items-center gap-2.5">
        <Plug className="size-4 shrink-0 text-muted-foreground" />
        <span className="min-w-0 break-words font-mono text-sm font-medium [overflow-wrap:anywhere] sm:truncate">
          {server.id}
        </span>
      </div>
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2 sm:contents">
        <div className="flex min-w-0 items-center">
          {!server.enabledGlobal ? (
            <Badge variant="outline" className="text-muted-foreground">
              Off globally
            </Badge>
          ) : null}
        </div>
        <div className="flex h-10 shrink-0 items-center justify-end gap-1.5 sm:h-auto sm:gap-2">
          <label
            htmlFor={switchId}
            className="cursor-pointer text-xs font-medium text-muted-foreground sm:hidden"
          >
            On for this bot
          </label>
          <Switch
            id={switchId}
            aria-label={`Allow ${server.id}`}
            checked={server.enabledForBot}
            disabled={!server.enabledGlobal || props.disabled}
            onCheckedChange={props.onSetEnabled}
          />
        </div>
      </div>
    </li>
  )
}

interface BotMcpContentProps {
  readonly botId: string
}

function BotMcpContent(_props: BotMcpContentProps) {
  const machine = BotMcpOwner.useMachine()
  const send = useAtomSet(machine.send)

  useEffect(() => {
    send(BotMcpEvents.Refresh())
  }, [send])

  return (
    <MachineState machine={machine} path="">
      {({ value, states }) => {
        const isSaving = states.Mutate.state.path === 'Mutate.Saving'
        const { servers } = value.data
        if (servers.length === 0) {
          return (
            <EmptyLine>
              No MCP servers configured.{' '}
              <Link to="/settings/mcp" className="underline underline-offset-4">
                Add one in MCP
              </Link>
            </EmptyLine>
          )
        }
        return (
          <Section
            title="Servers"
            action={
              <Button variant="outline" size="sm" render={<Link to="/settings/mcp" />}>
                Manage global MCP
              </Button>
            }
          >
            <div>
              <BotMcpHeader />
              <ul className="flex flex-col divide-y">
                {servers.map(server => (
                  <BotMcpServerRow
                    key={server.id}
                    server={server}
                    disabled={isSaving}
                    onSetEnabled={enabled =>
                      send(BotMcpEvents.SetEnabled({ id: server.id, enabled }))
                    }
                  />
                ))}
              </ul>
            </div>
          </Section>
        )
      }}
    </MachineState>
  )
}

interface BotMcpBodyProps {
  readonly botId: string
}

/** Per-bot MCP server enable/disable controls rendered inside the bot SplitDetail. */
export function BotMcpBody(props: BotMcpBodyProps) {
  const instanceKey = useBotInstanceKey(props.botId)
  const atom = botMcpAtomFor(instanceKey)
  const result = useAtomValue(atom)
  const retry = useAtomRefresh(atom)

  return matchCached(result, {
    onInitial: () => <EmptyLine>Loading…</EmptyLine>,
    onFailure: failure => <PageError cause={failure.cause} onRetry={retry} />,
    onSuccess: value => (
      <BotMcpOwner.Provider input={value} scopeKey={instanceKey}>
        <BotMcpContent botId={props.botId} />
      </BotMcpOwner.Provider>
    ),
  })
}
