import { EmptyLine, PageError } from '~/components/layout/Page'
import { SplitDetail, SplitItem, SplitPage } from '~/components/layout/SplitPage'
import { DiscordIcon } from '~/components/DiscordIcon'

import {
  DiscordSettings,
  DiscordSettingsOwner,
  DiscordStatusBadge,
  DiscordStatusDot,
} from './DiscordSettings'

interface ChatPlatformsPageProps {
  readonly isListView: boolean
}

/** Chat platforms with their settings; Discord is the only platform today. */
export function ChatPlatformsPage(props: ChatPlatformsPageProps) {
  return (
    <DiscordSettingsOwner>
      {loadState => (
        <SplitPage
          title="Chat platforms"
          isListView={props.isListView}
          items={
            <SplitItem
              to="/settings/chat/discord"
              label="Discord"
              icon={<DiscordIcon className="size-4 shrink-0 text-[#5865F2]" />}
              isActive
              {...(loadState.kind === 'loaded' ? { trailing: <DiscordStatusDot /> } : {})}
            />
          }
        >
          <SplitDetail
            title="Discord"
            isListView={props.isListView}
            backTo="/settings/chat"
            backLabel="Chat platforms"
            {...(loadState.kind === 'loaded' ? { actions: <DiscordStatusBadge /> } : {})}
          >
            {loadState.kind === 'loaded' ? (
              <DiscordSettings />
            ) : loadState.kind === 'failed' ? (
              <PageError cause={loadState.cause} onRetry={loadState.onRetry} />
            ) : (
              <EmptyLine>Loading…</EmptyLine>
            )}
          </SplitDetail>
        </SplitPage>
      )}
    </DiscordSettingsOwner>
  )
}
