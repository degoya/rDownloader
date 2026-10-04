/**
 * The hand-over point for the LinkGrabber's own keys: `a` adds links, `e` enqueues everything,
 * `w` enqueues everything paused and `r` removes every link (1.8.1).
 *
 * The view hands its actions in while it is mounted and takes them back when it goes, so the
 * four keys do nothing on any other page — the pattern of `k` (`setClearCompletedAction`). A
 * module of its own rather than a setter in `shortcutDefinitions.ts`, for the reason
 * `indexerSearchFocus.ts` is one: the view imports this, and the catalogue's router import would
 * otherwise follow it into every test that renders the view.
 */
interface LinkGrabberActions {
  addLinks: () => void
  enqueueAll: () => void
  enqueuePaused: () => void
  clearAll: () => void
}

let actions: LinkGrabberActions | null = null

export function setLinkGrabberActions(next: LinkGrabberActions | null): void {
  actions = next
}

/** Runs one handed-in action, if the LinkGrabber is on the page. */
export function runLinkGrabberAction(name: keyof LinkGrabberActions): void {
  actions?.[name]()
}
