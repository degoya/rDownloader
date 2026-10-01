/**
 * The hand-over point for `f`, which puts the keyboard in the LinkGrabber's indexer search
 * (RD-180-19).
 *
 * The panel hands its focus in while it is mounted and takes it back when it goes, so `f` does
 * nothing anywhere else. Without an enabled indexer its field is disabled and the focus lands on
 * the hint's link to the indexer settings instead. A module of its own rather than a setter in `shortcutDefinitions.ts`, for the
 * reason `nzbImportRequest.ts` is one: the panel imports this, and the catalogue's router import
 * would otherwise follow it into every view and test that renders the panel.
 */
let focusAction: (() => void) | null = null

export function setIndexerSearchFocusAction(action: (() => void) | null): void {
  focusAction = action
}

/** Runs the handed-in focus, if the search panel is on the page. */
export function focusIndexerSearch(): void {
  focusAction?.()
}
