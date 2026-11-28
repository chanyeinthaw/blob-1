import type { BotSkill } from '@blob-1/contracts'
import { useAtomRefresh, useAtomValue } from '@effect/atom-react'
import Markdown from 'react-markdown'
import remarkGfm from 'remark-gfm'

import { EmptyLine, focusRingClassName, PageError } from '~/components/layout/Page'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
  SheetTrigger,
} from '~/components/ui/sheet'
import { cn } from '~/lib/utils'
import { matchCached } from '~/lib/useBackgroundRefresh'

import { botSkillDetailAtomFor } from './botsState'

interface SkillDetailBodyProps {
  readonly instanceKey: string
  readonly name: string
}

function SkillDetailBody(props: SkillDetailBodyProps) {
  const atom = botSkillDetailAtomFor(`${props.instanceKey}/${props.name}`)
  const result = useAtomValue(atom)
  const retry = useAtomRefresh(atom)
  return matchCached(result, {
    onInitial: () => <EmptyLine>Loading…</EmptyLine>,
    onFailure: failure => <PageError cause={failure.cause} onRetry={retry} />,
    onSuccess: detail => (
      <div className="space-y-4">
        <div className="chat-markdown text-sm leading-relaxed break-words [overflow-wrap:anywhere]">
          <Markdown remarkPlugins={[remarkGfm]}>{detail.content}</Markdown>
        </div>
        {detail.files.length === 0 ? null : (
          <div>
            <p className="mb-1.5 text-xs font-medium text-muted-foreground">Files</p>
            <ul className="space-y-0.5 font-mono text-xs text-muted-foreground">
              {detail.files.map(file => (
                <li key={file} className="break-all">
                  {file}
                </li>
              ))}
            </ul>
          </div>
        )}
      </div>
    ),
  })
}

interface SkillDetailSheetProps {
  readonly instanceKey: string
  readonly skill: BotSkill
}

/** The skill's name and description as a row button opening its full `SKILL.md` in a side drawer. */
export function SkillDetailSheet(props: SkillDetailSheetProps) {
  const { skill } = props
  return (
    <Sheet>
      <SheetTrigger
        render={
          <button
            type="button"
            aria-label={`Show ${skill.name}`}
            className={cn('min-w-0 cursor-pointer rounded-md text-left', focusRingClassName)}
          />
        }
      >
        <p className="truncate font-mono text-sm hover:underline">{skill.name}</p>
        {skill.description === '' ? null : (
          <p className="truncate text-xs text-muted-foreground">{skill.description}</p>
        )}
      </SheetTrigger>
      <SheetContent className="gap-0 sm:max-w-xl md:rounded-l-2xl">
        <SheetHeader className="border-b pr-12">
          <SheetTitle className="font-mono">{skill.name}</SheetTitle>
          <SheetDescription>{skill.description}</SheetDescription>
        </SheetHeader>
        <div className="min-h-0 flex-1 overflow-y-auto p-4">
          <SkillDetailBody instanceKey={props.instanceKey} name={skill.name} />
        </div>
      </SheetContent>
    </Sheet>
  )
}
