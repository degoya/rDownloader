/**
 * The hand-over point for `f`, the search of the page that is open: on the LinkGrabber it opens
 * the indexer search drawer with the keyboard in its field (RD-180-19, RD-1230-02), on Downloads
 * it focuses the list's name search (RD-190-21).
 *
 * The drawer or the list hands its action in while it is mounted and takes it back when it goes,
 * so `f` does nothing anywhere else; the two are never on one page. Without an enabled indexer
 * the drawer's field is disabled and the focus lands on the hint's link to the indexer settings
 * instead. A module of its own rather than a setter in `shortcutDefinitions.ts`, for the reason
 * `nzbImportRequest.ts` is one: the drawer imports this, and the catalogue's router import would
 * otherwise follow it into every view and test that renders the drawer.
 */
let focusAction: (() => void) | null = null

export function setIndexerSearchFocusAction(action: (() => void) | null): void {
  focusAction = action
}

/** Runs the handed-in action, if the drawer or the list is on the page. */
export function focusIndexerSearch(): void {
  focusAction?.()
}
