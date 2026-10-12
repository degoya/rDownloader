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
 * The title, on its own line of text inside Nuxt UI's flex heading, so it can end in an ellipsis
 * (RD-1250-10): the heading's own `truncate` clips a flex container's text without one, and at
 * 390 px the German title read "Downloac". The full name stays as `title`; the buttons keep their
 * room (`shrink-0` in the navbar's right slot).
 */
export const DOWNLOADS_NAV_TITLE = 'min-w-0 truncate'
