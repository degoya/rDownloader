/**
 * The hand-over point for `a` on Downloads: it opens the direct job's dialog (RD-1220-03).
 *
 * `a` is "add" on both pages that take links — the LinkGrabber's *Add links* and this — the way
 * `f` is the search of whichever page is open; the two are never on one page. The view hands its
 * action in while it is mounted and takes it back when it goes. A module of its own for the reason
 * `linkGrabberActions.ts` is one: the catalogue's router import stays out of the view's tests.
 */
let openAction: (() => void) | null = null

export function setDirectAddAction(action: (() => void) | null): void {
  openAction = action
}

/** Opens the handed-in dialog; `false` when Downloads is not on the page. */
export function openDirectAdd(): boolean {
  if (!openAction) return false
  openAction()
  return true
}
