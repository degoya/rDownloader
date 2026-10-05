// The widget captcha flow's alarm and runtime message names (RD-108-02), shared by the
// background, the popup and the hoster's tab through captcha.js.

/** Name of the alarm that drives polling; `alarms` survives service-worker restarts. */
export const POLL_ALARM = 'rdownloader-captcha-poll'

/** Chrome's smallest alarm period. Two small requests a minute against a local server. */
export const POLL_PERIOD_MINUTES = 0.5

/** Runtime message a harvested token travels in, from the hoster's tab to the background. */
export const TOKEN_MESSAGE = 'rdownloader:captcha-token'

/** Runtime message from the popup: the origin is granted, open the hoster's page. */
export const OPEN_MESSAGE = 'rdownloader:captcha-open'

/** Runtime message from the hoster's tab: the page shows no widget at all (RD-120-45). */
export const NO_WIDGET_MESSAGE = 'rdownloader:captcha-no-widget'

/** Runtime message from the popup: a grant is about to be asked for (or was refused). */
export const GRANT_MESSAGE = 'rdownloader:captcha-grant'

/** Runtime message from the popup: run a poll and hand back what it found. */
export const POLL_MESSAGE = 'rdownloader:captcha-poll'

/** Runtime message from the popup: decline this widget. */
export const DECLINE_MESSAGE = 'rdownloader:captcha-decline'

/**
 * Every message type the background's `onMessage` answers.
 *
 * `rdownloader:captcha-decline` used to be answered by a branch of its own in `background.js`,
 * without the `fromOwnPage` check the other two got. The inconsistency stood there unexplained;
 * the message now goes through the same handler and the same check as the rest (RD-109-23).
 */
export const MESSAGE_TYPES = [TOKEN_MESSAGE, NO_WIDGET_MESSAGE, OPEN_MESSAGE, GRANT_MESSAGE, POLL_MESSAGE, DECLINE_MESSAGE]
