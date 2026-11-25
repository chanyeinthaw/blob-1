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

/** Cache-busted URL of a bot's custom avatar, or undefined while it uses the generated one. */
export function botAvatarUrl(bot: { readonly id: string; readonly avatarAt: number | null }) {
  return bot.avatarAt === null ? undefined : `/api/bots/${bot.id}/avatar?v=${bot.avatarAt}`
}

interface BotAvatarProps {
  readonly seed: string
  readonly imageUrl?: string | undefined
  readonly className?: string
}

/** A bot's custom avatar image, or the DiceBear `clay` avatar seeded by instance as the default face. */
export function BotAvatar(props: BotAvatarProps) {
  if (props.imageUrl !== undefined) {
    return (
      <img
        aria-hidden="true"
        alt=""
        src={props.imageUrl}
        className={cn('shrink-0 object-cover', props.className)}
      />
    )
  }
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
