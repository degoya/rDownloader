import { computed, onScopeDispose, ref, type ComputedRef } from 'vue'

/**
 * Which enlarged cover is open — exactly one, for the whole application (RD-107-16).
 *
 * Every row used to decide for itself, so two covers stood open at once: a pinned one in one
 * row and a hovered one in another, and a fast traversal where the leaving row's `mouseleave`
 * arrived after the next row's `mouseenter`. Rows cannot settle that between themselves, and
 * no common ancestor can either: hits are drawn in two places (the LinkGrabber's review list
 * and the archive under an expanded subscription), both can be on screen at once, and a
 * windowed list creates and destroys rows as it scrolls. The one thing every row can agree on
 * is module state outside the tree, so that is where "which row is open" lives. A row claims
 * the slot by identity, and only ever releases it while it still holds it — which is what
 * makes the late `mouseleave` harmless.
 *
 * **Pinning against hovering.** The two entrances are not equal and are kept in separate
 * slots. Pinning is a person's stated decision; a pointer crossing the list is not. So a
 * pointer or focus takes the single slot only for as long as it lasts, and when it ends the
 * pinned cover comes back — the pin is covered, never discarded. Only another deliberate act
 * (a tap on a different thumbnail) replaces a pin, and `Escape`, a second tap or a click
 * outside gives it up. `design.md` carries the rule and the reason.
 */

/** The row whose cover a pointer or keyboard focus is holding open right now. */
const transient = ref<symbol | null>(null)
/** The row whose cover a person pinned; it shows again once the transient one lets go. */
const pinned = ref<symbol | null>(null)

/** The single open row: whatever is being pointed at, else whatever was pinned. */
const openRow = computed<symbol | null>(() => transient.value ?? pinned.value)

export interface CoverPreview {
  /** True for exactly one row in the application at a time. */
  open: ComputedRef<boolean>
  /** The pointer entered (`true`) or left (`false`) this row's trigger. */
  setHovered: (value: boolean) => void
  /** Keyboard focus reached (`true`) or left (`false`) this row's trigger. */
  setFocused: (value: boolean) => void
  /** Pins this row's cover: the deliberate choice, which replaces any other row's pin. */
  pin: () => void
  /** A tap: pins this row's cover, or gives the pin up when it already holds it. */
  toggle: () => void
  /** Escape, a click outside, or the row going away: this row claims nothing any more. */
  close: () => void
}

export function useCoverPreview(): CoverPreview {
  const row = Symbol('cover-row')
  /** The pointer and the keyboard are two ways into the same transient slot. */
  const hovered = ref(false)
  const focused = ref(false)

  function sync(): void {
    if (hovered.value || focused.value) transient.value = row
    // Only if this row still holds the slot: a slow `mouseleave` must not close its successor.
    else if (transient.value === row) transient.value = null
  }

  function setHovered(value: boolean): void {
    hovered.value = value
    sync()
  }

  function setFocused(value: boolean): void {
    focused.value = value
    sync()
  }

  function close(): void {
    hovered.value = false
    focused.value = false
    sync()
    if (pinned.value === row) pinned.value = null
  }

  function pin(): void {
    pinned.value = row
  }

  function toggle(): void {
    if (pinned.value === row) close()
    else pin()
  }

  // A windowed list destroys rows while they are open; a claim must not outlive its row.
  onScopeDispose(close)

  return {
    open: computed(() => openRow.value === row),
    setHovered,
    setFocused,
    pin,
    toggle,
    close
  }
}
