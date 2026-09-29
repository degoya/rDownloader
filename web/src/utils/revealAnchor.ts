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
export const HIGHLIGHT_MS = 2000

/** The first thing inside a field that takes typing or a click: input, select, switch, button. */
const FOCUSABLE = 'input:not([type="hidden"]):not([disabled]), textarea:not([disabled]), select:not([disabled]), button:not([disabled]), [tabindex]:not([tabindex="-1"])'

/** Ids are the registry's own constants (`[a-z0-9._]`), so no escaping is needed. */
export function anchorSelector(id: string): string {
  return `[data-settings-anchor="${id}"]`
}

function prefersReducedMotion(): boolean {
  return typeof window.matchMedia === 'function' && window.matchMedia('(prefers-reduced-motion: reduce)').matches
}

/** The element once it exists and is shown (a hidden tab's content is mounted but not visible). */
async function waitForAnchor(id: string, timeoutMs: number): Promise<HTMLElement | null> {
  const deadline = Date.now() + timeoutMs
  for (;;) {
    const element = document.querySelector<HTMLElement>(anchorSelector(id))
    if (element && !element.closest('[hidden], [data-state="inactive"]')) return element
    if (Date.now() >= deadline) return null
    await new Promise(resolve => setTimeout(resolve, 50))
  }
}

export async function revealAnchor(id: string, options: { focus: boolean, timeoutMs?: number }): Promise<boolean> {
  const element = await waitForAnchor(id, options.timeoutMs ?? 5000)
  if (!element) return false
  element.scrollIntoView({ block: 'center', behavior: prefersReducedMotion() ? 'auto' : 'smooth' })
  element.setAttribute('data-search-highlight', '')
  setTimeout(() => element.removeAttribute('data-search-highlight'), HIGHLIGHT_MS)
  if (options.focus) {
    const target = element.matches(FOCUSABLE) ? element : element.querySelector<HTMLElement>(FOCUSABLE)
    // No second jump: the scroll above already centred the field.
    target?.focus({ preventScroll: true })
  }
  return true
}
