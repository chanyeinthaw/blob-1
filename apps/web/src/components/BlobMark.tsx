import { cn } from '~/lib/utils'

const blobPath =
  'M43.6,-58.9C55.7,-49.6,64,-35.5,68.6,-20.1C73.3,-4.7,74.2,12,67.9,25.3C61.6,38.6,48,48.6,33.2,56.4C18.4,64.2,2.3,69.8,-14.4,69.4C-31.1,68.9,-48.3,62.3,-58.6,49.8C-68.9,37.3,-72.2,18.6,-70.8,1.4C-69.4,-15.8,-63.3,-31.6,-52.6,-41.4C-41.9,-51.2,-26.6,-55,-11.9,-61.8C2.8,-68.6,31.5,-68.2,43.6,-58.9Z'

interface BlobTileProps {
  readonly className?: string
}

/** The Blob-1 mark on its black tile. */
export function BlobTile(props: BlobTileProps) {
  return (
    <span
      className={cn(
        'grid size-8 shrink-0 place-items-center rounded-lg bg-primary text-primary-foreground',
        props.className,
      )}
    >
      <svg viewBox="-80 -80 160 160" aria-hidden="true" className="pointer-events-none size-1/2">
        <path d={blobPath} fill="currentColor" />
      </svg>
    </span>
  )
}
