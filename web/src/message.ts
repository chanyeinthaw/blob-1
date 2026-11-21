import { Schema } from 'effect'
import { defineMessageUnion } from 'foldkit/message'
import { UrlRequest } from 'foldkit/navigation'
import { Url } from 'foldkit/url'

import * as Login from './page/login'
import * as Chat from './page/settings/chat'
import * as Identity from './page/settings/identity'
import * as Mcp from './page/settings/mcp'
import * as Models from './page/settings/models'
import * as Providers from './page/settings/providers'
import * as Toast from './ui/toast'

export const Message = defineMessageUnion({
  ClickedLink: { request: UrlRequest },
  ChangedUrl: { url: Url },
  CompletedNavigateInternal: {},
  CompletedLoadExternal: {},
  CompletedScrollActiveNav: {},
  ClickedSignOut: {},
  CompletedSignOut: {},
  FailedSignOut: {},
  ReceivedApiStatus: { connected: Schema.Boolean },
  ReceivedUnauthorized: {},
  GotLoginMessage: { message: Login.Message },
  GotIdentityMessage: { message: Identity.Message },
  GotModelsMessage: { message: Models.Message },
  GotProvidersMessage: { message: Providers.Message },
  GotChatMessage: { message: Chat.Message },
  GotMcpMessage: { message: Mcp.Message },
  GotToastMessage: { message: Toast.Message },
})

export type Message = typeof Message.Type
