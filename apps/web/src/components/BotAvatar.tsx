import { Avatar, Style } from '@dicebear/core'
import clayDefinition from '@dicebear/styles/clay.json'

import { cn } from '~/lib/utils'

const style = new Style(clayDefinition)
const svgCache = new Map<string, string>()

/** Deterministic DiceBear `clay` SVG for a bot instance (`<botId>@<generation>`). */
function botAvatarSvg(seed: string): string {
  let svg = svgCache.get(seed)
  if (svg === undefined) {
    svg = new Avatar(style, { seed }).toString()
    svgCache.set(seed, svg)
  }
  return svg
}

interface BotAvatarProps {
  readonly seed: string
  readonly className?: string
}

/** A bot's DiceBear `blobs` avatar, seeded by instance so a recreated bot gets a fresh face. */
export function BotAvatar(props: BotAvatarProps) {
  return (
    <span
      aria-hidden="true"
      className={cn(
        'grid shrink-0 place-items-center overflow-hidden [&_svg]:size-full',
        props.className,
      )}
      dangerouslySetInnerHTML={{ __html: botAvatarSvg(props.seed) }}
    />
  )
}
