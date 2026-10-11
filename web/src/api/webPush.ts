/**
 * Web Push for the installed app (RD-1240-13): the key this browser subscribes with, and the
 * browsers the service sends push messages to. Through the coded `call`, so a refusal keeps its
 * code; the shapes are the generated schema's.
 */
import { call } from './call'
import type { components } from './schema'

type Schemas = components['schemas']

/** A browser that receives push messages; its message keys never come back. */
export type WebPushSubscription = Schemas['WebPushSubscription']

export type WebPushSubscriptionRequest = Schemas['WebPushSubscriptionRequest']

export const webPushKey = () =>
  call<Schemas['WebPushKeyResponse']>('GET', '/api/v1/notifications/web-push/key')

export const listWebPushSubscriptions = () =>
  call<WebPushSubscription[]>('GET', '/api/v1/notifications/web-push/subscriptions')

/** Stores this browser's subscription; the same push address again updates it. */
export const saveWebPushSubscription = (body: WebPushSubscriptionRequest) =>
  call<WebPushSubscription>('POST', '/api/v1/notifications/web-push/subscriptions', body)

export const deleteWebPushSubscription = (id: string) =>
  call<unknown>('DELETE', `/api/v1/notifications/web-push/subscriptions/${encodeURIComponent(id)}`)
