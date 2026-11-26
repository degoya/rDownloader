/**
 * The Downloads navbar's widths (RD-1240-33, RD-1260-06).
 *
 * The navbar measures its own width (`@container`), as the LinkGrabber's does. Its actions are
 * icons at every width, each named by its `aria-label` and its tooltip (`NavbarAction`), so the
 * row is short; what only says again what is elsewhere — the file count, the time a pause holds
 * until — goes on a narrow navbar, as the LinkGrabber's less used actions do.
 */
export const DOWNLOADS_NAV_WIDE = 'hidden @min-[40rem]:inline-flex'
/**
 * What holds the queue — a schedule's pause, an account's traffic — in the navbar where there is
 * room, and on a line of its own under it where there is not: up to 1280 px of window the two
 * lay over the sidebar's button and left the title no width (WCAG 1.4.10, RD-1480-17).
 */
export const DOWNLOADS_NAV_NOTICES = 'hidden @min-[80rem]:contents'
export const DOWNLOADS_NOTICE_LINE = 'flex flex-wrap items-center gap-2 border-b border-default px-4 py-2 not-has-[>*]:hidden @min-[80rem]:hidden'
/**
 * The title, on its own line of text inside Nuxt UI's flex heading, so it can end in an ellipsis
 * (RD-1250-10): the heading's own `truncate` clips a flex container's text without one, and at
 * 390 px the German title read "Downloac". The full name stays as `title`; the buttons keep their
 * room (`shrink-0` in the navbar's right slot).
 */
export const DOWNLOADS_NAV_TITLE = 'min-w-0 truncate'
