/**
 * The hand-over point for `f`, which puts the keyboard in the LinkGrabber's indexer search
 * (RD-180-19).
 *
 * The search panel exists only while at least one indexer is enabled; it hands its focus in
 * while its field is on the page and takes it back when the field goes, so `f` does nothing
 * anywhere else. A module of its own rather than a setter in `shortcutDefinitions.ts`, for the
 * reason `nzbImportRequest.ts` is one: the panel imports this, and the catalogue's router import
 * would otherwise follow it into every view and test that renders the panel.
 */
let focusAction: (() => void) | null = null

export function setIndexerSearchFocusAction(action: (() => void) | null): void {
  focusAction = action
}

/** Runs the handed-in focus, if a search field is on the page. */
export function focusIndexerSearch(): void {
  focusAction?.()
}
