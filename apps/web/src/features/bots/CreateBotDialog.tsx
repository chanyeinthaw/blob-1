import { useAtomSet } from '@effect/atom-react'
import { useNavigate } from '@tanstack/react-router'
import { AtomMachine } from '@typeonce/effect-machine/reactivity'
import { useMachineResult } from '@typeonce/effect-machine-react'
import { Option } from 'effect'
import { AsyncResult } from 'effect/reactivity'
import { type FormEvent, useEffect, useState } from 'react'

import { FieldError, FieldRow } from '~/components/layout/Page'
import { Button } from '~/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '~/components/ui/dialog'
import { Input } from '~/components/ui/input'
import { MachineAtoms, makeBotInstanceKey } from '~/lib/runtime'
import { createPersistentMachineContext } from '~/lib/shellMachines'

import { BotsEvents, type BotsInput, BotsMachine } from './botsMachine'

export const BotsOwner = createPersistentMachineContext(
  'bots',
  ({ input }: { readonly input: BotsInput }) => MachineAtoms.make(BotsMachine, input),
)

export function useBotsRoot() {
  const machine = BotsOwner.useMachine()
  const result = useMachineResult(AtomMachine.select(machine, ''))
  return AsyncResult.match(result, {
    onInitial: () => undefined,
    onFailure: () => undefined,
    onSuccess: ({ value }) => Option.getOrUndefined(value),
  })
}

/** Returns an immutable instance key (`<botId>@<generation>`) so recreated slugs never reuse stale cached state. */
export function useBotInstanceKey(botId: string): string {
  const bots = useBotsRoot()?.bots ?? []
  const bot = bots.find(entry => entry.id === botId)
  return makeBotInstanceKey(botId, bot?.generation)
}

const slugify = (raw: string): string =>
  raw
    .toLowerCase()
    .trim()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 32)

interface CreateBotFormProps {
  readonly onClose: () => void
}

function CreateBotForm(props: CreateBotFormProps) {
  const machine = BotsOwner.useMachine()
  const send = useAtomSet(machine.send)
  const root = useBotsRoot()
  const navigate = useNavigate()

  const [name, setName] = useState('')
  const [id, setId] = useState('')
  const [idTouched, setIdTouched] = useState(false)

  const lastCreatedId = root?.lastCreatedId ?? null
  const createError = root?.createError ?? null
  const isCreating = root?.busyIds.includes(id.trim()) ?? false
  const { onClose } = props

  useEffect(() => {
    if (lastCreatedId === null) return
    onClose()
    send(BotsEvents.ClearCreateState())
    void navigate({
      to: '/settings/bots/$botId/chat',
      params: { botId: lastCreatedId },
    })
  }, [lastCreatedId, navigate, onClose, send])

  const handleNameChange = (value: string) => {
    setName(value)
    if (!idTouched) {
      setId(slugify(value))
    }
  }

  const handleSubmit = (event: FormEvent) => {
    event.preventDefault()
    const cleanId = id.trim()
    const cleanName = name.trim()
    if (cleanId === '' || cleanName === '') return
    send(BotsEvents.Create({ id: cleanId, name: cleanName }))
  }

  return (
    <form onSubmit={handleSubmit} className="space-y-2">
      <FieldRow id="new-bot-name" label="Name">
        <Input
          id="new-bot-name"
          placeholder="Sunday"
          value={name}
          onChange={event => handleNameChange(event.target.value)}
        />
      </FieldRow>
      <FieldRow id="new-bot-id" label="ID">
        <Input
          id="new-bot-id"
          placeholder="sunday"
          value={id}
          onChange={event => {
            setIdTouched(true)
            setId(event.target.value.toLowerCase())
          }}
          className="font-mono text-sm"
        />
      </FieldRow>
      {createError !== null ? <FieldError>{createError}</FieldError> : null}
      <DialogFooter className="pt-2">
        <Button type="button" variant="ghost" onClick={props.onClose}>
          Cancel
        </Button>
        <Button type="submit" disabled={isCreating || name.trim() === '' || id.trim() === ''}>
          {isCreating ? 'Creating…' : 'Create'}
        </Button>
      </DialogFooter>
    </form>
  )
}

interface CreateBotDialogProps {
  readonly open: boolean
  readonly onOpenChange: (open: boolean) => void
}

/** Modal dialog for creating a new bot inside this instance. */
export function CreateBotDialog(props: CreateBotDialogProps) {
  const machine = BotsOwner.useMachine()
  const send = useAtomSet(machine.send)

  const handleOpenChange = (next: boolean) => {
    props.onOpenChange(next)
    if (!next) {
      send(BotsEvents.ClearCreateState())
    }
  }

  return (
    <Dialog open={props.open} onOpenChange={handleOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>New bot</DialogTitle>
        </DialogHeader>
        {props.open ? <CreateBotForm onClose={() => handleOpenChange(false)} /> : null}
      </DialogContent>
    </Dialog>
  )
}
