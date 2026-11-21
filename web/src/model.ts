import { Schema } from 'effect'
import { defineTaggedUnion, taggedStruct } from 'foldkit/schema'

import * as Login from './page/login'
import * as Chat from './page/settings/chat'
import * as Identity from './page/settings/identity'
import * as Mcp from './page/settings/mcp'
import * as Models from './page/settings/models'
import * as Providers from './page/settings/providers'
import * as Toast from './ui/toast'

export const Connection = defineTaggedUnion({
  Connected: {},
  Disconnected: {},
})
export type Connection = typeof Connection.Type

export const Page = defineTaggedUnion({
  Identity: { model: Identity.Model },
  Models: { model: Models.Model },
  Providers: { model: Providers.Model },
  Chat: { model: Chat.Model },
  Mcp: { model: Mcp.Model },
})
export type Page = typeof Page.Type

export const SignedOut = taggedStruct('SignedOut', {
  login: Login.Model,
})
export type SignedOut = typeof SignedOut.Type

export const SignedIn = taggedStruct('SignedIn', {
  connection: Connection,
  toast: Toast.Model,
  page: Page,
})
export type SignedIn = typeof SignedIn.Type

export const Model = Schema.Union([SignedOut, SignedIn])
export type Model = typeof Model.Type
