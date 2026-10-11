/**
 * When the Downloads navbar's buttons carry their words (RD-1240-33).
 *
 * The navbar measures its own width (`@container`), as the LinkGrabber's does: the labelled row
 * needs about 1150 px beside the title and the sidebar toggle, and below that it squeezed the
 * title to its first letter and, at 1024 px, pushed the toggle and the title out and cut "Clear
 * list" off at the window's edge. Below the threshold every button keeps only its icon, its name
 * staying as `aria-label` and `title`; the file count goes, the toolbar repeats it.
 */
export const DOWNLOADS_NAV_LABEL = { label: 'hidden @min-[72rem]:inline' }
/** The parts that only say again what is elsewhere: the file count. */
export const DOWNLOADS_NAV_WIDE = 'hidden @min-[72rem]:inline-flex'
/** The key hint, first to go. */
export const DOWNLOADS_NAV_KBD = 'hidden @min-[80rem]:inline-flex'
