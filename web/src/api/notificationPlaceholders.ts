/**
 * The placeholders a notification rule's own title and text may name (RD-1280-02), and which
 * event fills which: the service's one table (`rd_notify`), read by the rule editor for its list
 * and its preview. The types are written by hand until the schema is regenerated from the
 * service; the path is cast for the same reason.
 */
import type { NotificationEvent } from './types'
import { call, type CallPath } from './call'

/** One placeholder, written `{name}`, with the sample value the preview fills it with. */
export interface NotificationPlaceholder {
  name: string
  /** `time` and `date` are the service's clock, in its zone and format. */
  example: string
}

/** What one event fills. */
export interface NotificationEventPlaceholders {
  event: NotificationEvent
  takes_template: boolean
  placeholders: string[]
  /**
   * True for the events whose bursts are one notification each, worded when the burst closes
   * (RD-1291-01); `file`, `package` and `category` are filled only for a burst of one.
   */
  in_bursts: boolean
  /** Sample values the preview takes for this event in place of the table's: a burst of three. */
  examples: NotificationPlaceholder[]
}

export interface NotificationPlaceholderTable {
  placeholders: NotificationPlaceholder[]
  events: NotificationEventPlaceholders[]
}

/** The rule fields the regenerated schema will carry; `null` or absent sends the built-in wording. */
export interface NotificationRuleWording {
  title_template?: string | null
  body_template?: string | null
}

const PATH = '/api/v1/notifications/placeholders' as CallPath

export const fetchNotificationPlaceholders = () => call<NotificationPlaceholderTable>('GET', PATH)
