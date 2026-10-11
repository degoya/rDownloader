/**
 * The hand-over point for `f`, the LinkGrabber's indexer search: it opens the search drawer with
 * the keyboard in its field (RD-180-19, RD-1230-02), from every page (owner, 2026-10-10).
 *
 * The drawer hands its action in while it is mounted and takes it back when it goes. Pressed
 * elsewhere, `f` asks for the search and goes to the LinkGrabber, and the drawer answers the
 * request when it hands its action in. Without an enabled indexer the drawer's field is disabled
 * and the focus lands on the hint's link to the indexer settings instead. A module of its own
 * rather than a setter in `shortcutDefinitions.ts`, for the reason `nzbImportRequest.ts` is one:
 * the drawer imports this, and the catalogue's router import would otherwise follow it into every
 * view and test that renders the drawer.
 */
let focusAction: (() => void) | null = null
let requested = false

export function setIndexerSearchFocusAction(action: (() => void) | null): void {
  focusAction = action
  if (action && requested) {
    requested = false
    action()
  }
}

/**
 * Runs the handed-in action and answers `true`; without the drawer on the page, records the
 * request for the drawer to answer once it is mounted and answers `false`.
 */
export function focusIndexerSearch(): boolean {
  if (focusAction) {
    focusAction()
    return true
  }
  requested = true
  return false
}

/** Drops a request the drawer never answered — the way to the LinkGrabber was refused. */
export function cancelIndexerSearchRequest(): void {
  requested = false
}
