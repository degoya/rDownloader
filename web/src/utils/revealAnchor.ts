/**
 * Brings a settings card or field the search found into view (RD-170-15): waits until the page
 * has rendered it, scrolls it to the middle, marks it for a moment and, for a field, moves the
 * focus into it.
 *
 * Waiting is needed because the search navigates first: the settings view is a lazy route, and
 * a card may render only after its page has loaded its data. The wait gives up after `timeoutMs`
 * rather than holding on to a page somebody has already left.
 */

/** How long the mark stays; `main.css` fades it over the same time. */
const HIGHLIGHT_MS = 2000

/**
 * How long the found element is kept in view while the page settles (RD-1240-33). A card above
 * it can finish loading after the scroll — the bandwidth status above the limits did, opened from
 * another settings page — and push the field out of view again; four runs of four ended with the
 * field focused below the window.
 */
const SETTLE_MS = 2000
const SETTLE_POLL_MS = 100

/** The first thing inside a field that takes typing or a click: input, select, switch, button. */
const FOCUSABLE = 'input:not([type="hidden"]):not([disabled]), textarea:not([disabled]), select:not([disabled]), button:not([disabled]), [tabindex]:not([tabindex="-1"])'

/** Ids are the registry's own constants (`[a-z0-9._]`), so no escaping is needed. */
function anchorSelector(id: string): string {
  return `[data-settings-anchor="${id}"]`
}

function prefersReducedMotion(): boolean {
  return typeof window.matchMedia === 'function' && window.matchMedia('(prefers-reduced-motion: reduce)').matches
}

/** The element once it exists and is shown (a hidden tab's content is mounted but not visible). */
async function waitForAnchor(id: string, timeoutMs: number): Promise<HTMLElement | null> {
  const deadline = Date.now() + timeoutMs
  for (;;) {
    // A page left (or a test environment torn down) while the poll waits: nothing to reveal.
    if (typeof document === 'undefined') return null
    const element = document.querySelector<HTMLElement>(anchorSelector(id))
    if (element && !element.closest('[hidden], [data-state="inactive"]')) return element
    if (Date.now() >= deadline) return null
    await new Promise(resolve => setTimeout(resolve, 50))
  }
}

/** The nearest ancestor that scrolls, or the document's own scroller. */
function scrollerOf(element: HTMLElement): HTMLElement | null {
  for (let parent = element.parentElement; parent; parent = parent.parentElement) {
    const { overflowY } = getComputedStyle(parent)
    if ((overflowY === 'auto' || overflowY === 'scroll') && parent.scrollHeight > parent.clientHeight) return parent
  }
  return document.scrollingElement as HTMLElement | null
}

/** Where the element sits in its scroller's content, independent of how far it is scrolled. */
function contentOffset(element: HTMLElement, scroller: HTMLElement | null): number {
  return element.getBoundingClientRect().top + (scroller?.scrollTop ?? 0)
}

/**
 * Scrolls again whenever the content above the element moves it, until the page has settled or
 * the reader scrolls, types or clicks — their own movement is never taken back.
 */
function keepInView(element: HTMLElement, behavior: ScrollBehavior): void {
  const scroller = scrollerOf(element)
  let placed = contentOffset(element, scroller)
  let stopped = false
  const stop = (): void => {
    stopped = true
    for (const kind of ['wheel', 'touchstart', 'keydown', 'pointerdown'] as const) window.removeEventListener(kind, stop)
  }
  for (const kind of ['wheel', 'touchstart', 'keydown', 'pointerdown'] as const) window.addEventListener(kind, stop, { passive: true })
  const deadline = Date.now() + SETTLE_MS
  const check = (): void => {
    if (stopped || !element.isConnected || Date.now() >= deadline) {
      stop()
      return
    }
    const now = contentOffset(element, scroller)
    if (Math.abs(now - placed) > 1) {
      placed = now
      element.scrollIntoView({ block: 'center', behavior })
    }
    setTimeout(check, SETTLE_POLL_MS)
  }
  setTimeout(check, SETTLE_POLL_MS)
}

export async function revealAnchor(id: string, options: { focus: boolean, timeoutMs?: number }): Promise<boolean> {
  const element = await waitForAnchor(id, options.timeoutMs ?? 5000)
  if (!element) return false
  const behavior: ScrollBehavior = prefersReducedMotion() ? 'auto' : 'smooth'
  element.scrollIntoView({ block: 'center', behavior })
  keepInView(element, behavior)
  element.setAttribute('data-search-highlight', '')
  setTimeout(() => element.removeAttribute('data-search-highlight'), HIGHLIGHT_MS)
  if (options.focus) {
    const target = element.matches(FOCUSABLE) ? element : element.querySelector<HTMLElement>(FOCUSABLE)
    // No second jump: the scroll above already centred the field.
    target?.focus({ preventScroll: true })
  }
  return true
}
