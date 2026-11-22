/**
 * A hit of an indexer subscription handed straight to a remote-job provider (RD-1460-03):
 * `POST /api/v1/subscriptions/items/{id}/remote-job`.
 *
 * Through the shared coded `call`, so a refusal keeps its code. The types are written by hand
 * until the schema is regenerated from the service; the path is cast for the same reason.
 */
import type { RemoteJob, SubscriptionItemState } from './types'
import { call, type CallPath } from './call'

export interface SubscriptionItemHandOver {
  item_id: string
  /** `queued`: the hit is decided as after "queue". */
  state: SubscriptionItemState
  job: RemoteJob
  /** The account already had a job for this NZB: nothing was sent, the hit is decided anyway. */
  already_running: boolean
}

/**
 * The settings switch the regenerated schema will carry (default on). A type, not an interface:
 * the defaults are also read as a plain record, which an interface's missing index refuses.
 */
export type IndexerHandOverSettings = {
  nzb_hand_over_indexers_enabled?: boolean
}

export const handOverSubscriptionItem = (itemId: string, accountId: string) =>
  call<SubscriptionItemHandOver>(
    'POST',
    `/api/v1/subscriptions/items/${encodeURIComponent(itemId)}/remote-job` as CallPath,
    { account_id: accountId }
  )
