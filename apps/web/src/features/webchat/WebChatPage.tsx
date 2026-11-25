import type { ChatMessage } from '@blob-1/contracts'
import { useAtomRefresh, useAtomSet, useAtomValue } from '@effect/atom-react'
import { Link } from '@tanstack/react-router'
import { AtomMachine } from '@typeonce/effect-machine/reactivity'
import { MachineState, useMachineResult } from '@typeonce/effect-machine-react'
import { AsyncResult } from 'effect/reactivity'
import { ArrowDown, ArrowUp, ChevronRight } from 'lucide-react'
import {
  type FormEvent,
  type KeyboardEvent,
  type ReactNode,
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from 'react'
import Markdown from 'react-markdown'
import remarkGfm from 'remark-gfm'

import { BotAvatar, botAvatarUrl } from '~/components/BotAvatar'
import { EmptyLine, PageError, panelClassName } from '~/components/layout/Page'
import { Button } from '~/components/ui/button'
import { useBotInstanceKey, useBotsRoot } from '~/features/bots/CreateBotDialog'
import { machineAtomsForBot } from '~/lib/runtime'
import { createPersistentMachineContext } from '~/lib/shellMachines'
import { matchCached, useChangeRefresh } from '~/lib/useBackgroundRefresh'
import { cn } from '~/lib/utils'

import { WebChatEvents, type WebChatInput, WebChatMachine } from './webChatMachine'
import { webChatAtomFor } from './webChatState'

const WebChatOwner = createPersistentMachineContext(
  'webchat',
  ({ input, scopeKey }: { readonly input: WebChatInput; readonly scopeKey: string }) =>
    machineAtomsForBot(scopeKey).make(WebChatMachine, input),
)

/** Distance from the top that loads older messages, and from the bottom that still counts as "at the end". */
const EDGE_PX = 120

interface WebChatPageProps {
  readonly botId: string
}

/** A bot's web chat: the default page when a bot is selected. */
export function WebChatPage(props: WebChatPageProps) {
  const instanceKey = useBotInstanceKey(props.botId)
  const atom = webChatAtomFor(instanceKey)
  const result = useAtomValue(atom)
  const retry = useAtomRefresh(atom)
  const bots = useBotsRoot()?.bots ?? []
  const bot = bots.find(entry => entry.id === props.botId)
  const botName = bot?.name ?? props.botId

  const frame = { botId: props.botId, botName, instanceKey, imageUrl: bot && botAvatarUrl(bot) }

  return matchCached(result, {
    onInitial: () => (
      <ChatFrame {...frame}>
        <EmptyLine>Loading…</EmptyLine>
      </ChatFrame>
    ),
    onFailure: failure => (
      <ChatFrame {...frame}>
        <PageError cause={failure.cause} onRetry={retry} />
      </ChatFrame>
    ),
    onSuccess: page => (
      <ChatFrame {...frame}>
        <WebChatOwner.Provider input={page} scopeKey={instanceKey}>
          <ChatBody instanceKey={instanceKey} />
        </WebChatOwner.Provider>
      </ChatFrame>
    ),
  })
}

interface ChatFrameProps extends ChatHeaderProps {
  readonly children: ReactNode
}

/** The full-page panel with the floating bot button on top. */
function ChatFrame(props: ChatFrameProps) {
  return (
    // On mobile the shell grows with its content, so the chat bounds itself
    // to the viewport below the 53px bot bar and the list owns scrolling.
    <div className={cn(panelClassName, 'relative flex-col max-md:h-[calc(100dvh-53px)]')}>
      <ChatHeader
        botId={props.botId}
        botName={props.botName}
        instanceKey={props.instanceKey}
        imageUrl={props.imageUrl}
      />
      {props.children}
    </div>
  )
}

interface ChatHeaderProps {
  readonly botId: string
  readonly botName: string
  readonly instanceKey: string
  readonly imageUrl: string | undefined
}

/** The bot's name over a fade, so messages scroll under it; it opens the bot's settings. */
function ChatHeader(props: ChatHeaderProps) {
  return (
    <div className="pointer-events-none absolute inset-x-0 top-0 z-10 bg-gradient-to-b from-background from-60% to-transparent px-4 pt-3 pb-6">
      <div className="mx-auto w-full max-w-3xl">
        <Link
          to="/settings/bots/$botId"
          params={{ botId: props.botId }}
          aria-label={`${props.botName} settings`}
          className="pointer-events-auto -ml-2 inline-flex max-w-full items-center gap-2.5 rounded-xl py-1.5 pr-2 pl-2 text-foreground transition-colors hover:bg-secondary"
        >
          <BotAvatar
            seed={props.instanceKey}
            imageUrl={props.imageUrl}
            className="size-8 rounded-lg"
          />
          <span className="truncate text-base font-semibold tracking-tight">{props.botName}</span>
          <ChevronRight className="size-4 shrink-0 text-muted-foreground" />
        </Link>
      </div>
    </div>
  )
}

interface ChatBodyProps {
  readonly instanceKey: string
}

function ChatBody(props: ChatBodyProps) {
  const machine = WebChatOwner.useMachine()
  const send = useAtomSet(machine.send)
  useEffect(() => {
    send(WebChatEvents.Refresh())
  }, [send])
  const refresh = useCallback(() => send(WebChatEvents.Refresh()), [send])
  useChangeRefresh(props.instanceKey, 'chat', refresh)

  const sending = AsyncResult.getOrElse(
    useMachineResult(AtomMachine.matches(machine, 'Send.Working')),
    () => false,
  )
  const [composerHeight, setComposerHeight] = useState(0)

  return (
    <MachineState machine={machine} path="">
      {({ value }) => (
        <>
          <MessageList
            messages={value.messages}
            hasMore={value.hasMore}
            typingUntil={value.typingUntil}
            composerHeight={composerHeight}
            onLoadOlder={() => send(WebChatEvents.LoadOlder())}
          />
          <Composer
            sentSeq={value.sentSeq}
            onResize={setComposerHeight}
            sending={sending}
            working={value.working}
            error={value.sendError}
            onSend={text => send(WebChatEvents.Send({ text }))}
            onEdit={() => {
              if (value.sendError !== null) send(WebChatEvents.ClearSendError())
            }}
          />
        </>
      )}
    </MachineState>
  )
}

interface MessageListProps {
  readonly messages: ReadonlyArray<ChatMessage>
  readonly hasMore: boolean
  readonly typingUntil: number | null
  /** The composer's input height; a change resizes the list. */
  readonly composerHeight: number
  readonly onLoadOlder: () => void
}

/** Messages oldest first; stays pinned to the end unless the user scrolled up. */
function MessageList(props: MessageListProps) {
  const scrollRef = useRef<HTMLDivElement>(null)
  const atEnd = useRef(true)
  const firstId = useRef<number | null>(null)
  const heightBefore = useRef(0)
  const [scrolledUp, setScrolledUp] = useState(false)
  const typing = useTyping(props.typingUntil)

  // Older messages prepend: keep the view where it was. New ones append:
  // follow them when the user was already at the end.
  useLayoutEffect(() => {
    const element = scrollRef.current
    if (element === null) return
    const first = props.messages[0]?.id ?? null
    if (firstId.current !== null && first !== null && first < firstId.current) {
      element.scrollTop += element.scrollHeight - heightBefore.current
    } else if (atEnd.current) {
      element.scrollTop = element.scrollHeight
    }
    firstId.current = first
    heightBefore.current = element.scrollHeight
  }, [props.messages])

  // The composer changing height resizes this viewport without a scroll
  // event: stay pinned to the end if the user was there.
  useLayoutEffect(() => {
    const element = scrollRef.current
    if (props.composerHeight > 0 && atEnd.current && element !== null) {
      element.scrollTop = element.scrollHeight
    }
  }, [props.composerHeight])

  // The typing bubble also grows the list; follow it at the end.
  useLayoutEffect(() => {
    const element = scrollRef.current
    if (typing && atEnd.current && element !== null) element.scrollTop = element.scrollHeight
  }, [typing])

  const onScroll = () => {
    const element = scrollRef.current
    if (element === null) return
    heightBefore.current = element.scrollHeight
    atEnd.current = element.scrollHeight - element.scrollTop - element.clientHeight < EDGE_PX
    setScrolledUp(!atEnd.current)
    if (element.scrollTop < EDGE_PX && props.hasMore) props.onLoadOlder()
  }

  const scrollToEnd = () => {
    const element = scrollRef.current
    element?.scrollTo({ top: element.scrollHeight, behavior: 'smooth' })
  }

  return (
    <div className="relative flex min-h-0 flex-1 flex-col">
      <div
        ref={scrollRef}
        onScroll={onScroll}
        className="min-h-0 flex-1 overflow-y-auto [scrollbar-gutter:stable_both-edges]"
      >
        <ol
          aria-label="Messages"
          aria-live="polite"
          className="mx-auto flex w-full max-w-3xl flex-col px-4 pt-20 pb-4"
        >
          {props.messages.map((message, index) => (
            <Bubble
              key={message.id}
              message={message}
              grouped={props.messages[index - 1]?.bot === message.bot}
            />
          ))}
          {typing ? (
            <li
              aria-label="Typing"
              className={cn(
                'flex items-center gap-1 self-start rounded-2xl bg-secondary px-4 py-3.5',
                props.messages.at(-1)?.bot === true ? 'mt-1.5' : 'mt-5',
              )}
            >
              <span className="typing-dot size-1.5 rounded-full bg-muted-foreground" />
              <span className="typing-dot size-1.5 rounded-full bg-muted-foreground [animation-delay:150ms]" />
              <span className="typing-dot size-1.5 rounded-full bg-muted-foreground [animation-delay:300ms]" />
            </li>
          ) : null}
        </ol>
      </div>
      {scrolledUp ? (
        <div className="pointer-events-none absolute inset-x-0 bottom-3 z-10 flex justify-center">
          <Button
            type="button"
            variant="outline"
            size="icon"
            aria-label="Scroll to the newest message"
            onClick={scrollToEnd}
            className="pointer-events-auto size-9 rounded-full bg-background shadow-sm md:size-9"
          >
            <ArrowDown />
          </Button>
        </div>
      ) : null}
    </div>
  )
}

/** Whether the bot shows as typing now; re-checks once when the signal lapses. */
function useTyping(typingUntil: number | null): boolean {
  const [now, setNow] = useState(() => Date.now() / 1000)
  useEffect(() => {
    if (typingUntil === null) return
    const wait = Math.max(0, typingUntil * 1000 - Date.now())
    const timer = window.setTimeout(() => setNow(Date.now() / 1000), wait + 50)
    return () => window.clearTimeout(timer)
  }, [typingUntil])
  return typingUntil !== null && typingUntil > now
}

interface BubbleProps {
  readonly message: ChatMessage
  /** Follows a message from the same side, so it sits closer. */
  readonly grouped: boolean
}

function Bubble(props: BubbleProps) {
  const { message } = props
  return (
    <li
      className={cn(
        'max-w-[85%] rounded-2xl px-4 py-2.5 text-sm leading-relaxed break-words [overflow-wrap:anywhere] first:mt-0',
        props.grouped ? 'mt-1.5' : 'mt-5',
        message.bot
          ? 'self-start bg-secondary text-foreground'
          : 'self-end bg-primary whitespace-pre-wrap text-primary-foreground',
      )}
    >
      {message.bot ? (
        <div className="chat-markdown">
          <Markdown remarkPlugins={[remarkGfm]}>{message.text}</Markdown>
        </div>
      ) : (
        message.text
      )}
    </li>
  )
}

interface ComposerProps {
  /** Bumps when a send succeeds. */
  readonly sentSeq: number
  /** Reports the input's new height after it grows or shrinks. */
  readonly onResize: (height: number) => void
  readonly sending: boolean
  readonly working: boolean
  readonly error: string | null
  readonly onSend: (text: string) => void
  readonly onEdit: () => void
}

/** Fits a textarea to its text, up to its CSS max height (Safari lacks `field-sizing`). */
function grow(input: HTMLTextAreaElement): number {
  input.style.height = 'auto'
  input.style.height = `${input.scrollHeight}px`
  return input.offsetHeight
}

/** The floating message box. Enter sends; Shift+Enter adds a line. */
function Composer(props: ComposerProps) {
  const [text, setText] = useState('')
  const canSend = text.trim() !== '' && !props.sending
  const inputRef = useRef<HTMLTextAreaElement>(null)
  const { onResize } = props
  const [submitted, setSubmitted] = useState<string | null>(null)

  // After a send lands, clear the box only if it still holds what was sent,
  // so text typed while sending survives.
  const [seenSeq, setSeenSeq] = useState(props.sentSeq)
  if (seenSeq !== props.sentSeq) {
    setSeenSeq(props.sentSeq)
    if (submitted !== null && text === submitted) setText('')
    setSubmitted(null)
  }
  // Shrink back once the box is cleared.
  useLayoutEffect(() => {
    if (text === '' && inputRef.current !== null) onResize(grow(inputRef.current))
  }, [text, onResize])

  const submit = (event?: FormEvent) => {
    event?.preventDefault()
    if (canSend) {
      setSubmitted(text)
      props.onSend(text)
    }
  }

  const onKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing) {
      event.preventDefault()
      submit()
    }
  }

  return (
    // Its own row under the list, so a growing draft never covers messages.
    <div className="shrink-0 px-4 pt-1 pb-4">
      <div className="mx-auto w-full max-w-3xl">
        {props.working || props.error !== null ? (
          <p
            role={props.error === null ? 'status' : 'alert'}
            className={cn(
              'mb-2 px-4 text-xs',
              props.error === null ? 'text-muted-foreground' : 'text-destructive',
            )}
          >
            {props.error ?? 'Working on a task…'}
          </p>
        ) : null}
        <form
          onSubmit={submit}
          className="flex items-end gap-2 rounded-3xl border border-border bg-background p-1.5 shadow-[0_1px_4px_rgb(0_0_0/0.06),0_8px_28px_rgb(0_0_0/0.10)]"
        >
          <textarea
            ref={inputRef}
            aria-label="Message"
            placeholder="Message"
            rows={1}
            value={text}
            onChange={event => {
              setText(event.target.value)
              props.onResize(grow(event.target))
              props.onEdit()
            }}
            onKeyDown={onKeyDown}
            className="block h-9 max-h-48 flex-1 resize-none overflow-y-auto bg-transparent px-3.5 py-2 text-sm leading-5 outline-none placeholder:text-muted-foreground"
          />
          <Button
            type="submit"
            size="icon"
            aria-label="Send"
            disabled={!canSend}
            className="size-9 shrink-0 rounded-full md:size-9"
          >
            <ArrowUp />
          </Button>
        </form>
      </div>
    </div>
  )
}
