import { ref, watch, type Ref } from 'vue'

/**
 * Range selection for a list with row checkboxes, the way a file explorer does it (RD-170-13).
 *
 * A plain or Ctrl/Cmd click toggles one row and makes it the anchor. A Shift+click — or
 * Shift+Space on a focused checkbox — sets every row from the anchor to the clicked one, in the
 * order the rows are on screen, to the state the clicked row has after the click. The anchor
 * stays put while the range is stretched, so a second Shift+click corrects the first.
 *
 * `order` is the visible order of the row keys (after sorting, filtering and collapsing), and
 * `apply` sets a batch of them. A group row — a package, a folder — names what it stands for
 * through `members`. At either end of a range it brings all of them along, as its own checkbox
 * would; inside a range it does so only while it is collapsed, because an open group's visible
 * rows are part of the range themselves and a hidden one is not "between" anything.
 *
 * The anchor is a key, not an index, and it is dropped the moment its row leaves `order`, so a
 * list that changed underneath never reaches from the wrong row.
 *
 * A checkbox reports its new value, not the event behind it, so the list listens in the capture
 * phase (`@click.capture="noteModifier"`, `@keydown.capture="noteModifier"`) and the flag is set
 * by the time the checkbox reports.
 */
export function useRangeSelection(
  order: Ref<readonly string[]>,
  apply: (keys: string[], selected: boolean) => void,
  members: (key: string) => readonly string[] | undefined = () => undefined
) {
  const anchor = ref<string | null>(null)
  /** Whether the pick about to arrive holds Shift. */
  const extending = ref(false)

  watch(order, (keys) => {
    if (anchor.value !== null && !keys.includes(anchor.value)) anchor.value = null
  })

  function noteModifier(event: MouseEvent | KeyboardEvent): void {
    if (event.type !== 'click') {
      extending.value = event.shiftKey
      return
    }
    // Space on a checkbox fires a click with `detail` 0, and whether that click carries the
    // modifiers depends on the browser; the keydown before it has already said.
    extending.value = event.shiftKey || ((event as MouseEvent).detail === 0 && extending.value)
  }

  function pick(key: string, selected: boolean, extend = extending.value): void {
    extending.value = false
    const keys = order.value
    const from = anchor.value === null ? -1 : keys.indexOf(anchor.value)
    const to = keys.indexOf(key)
    if (extend && from >= 0 && to >= 0) {
      const [low, high] = from <= to ? [from, to] : [to, from]
      const shown = new Set(keys)
      apply(keys.slice(low, high + 1).flatMap((row, index) => {
        const group = members(row)
        if (!group) return [row]
        const end = index === 0 || index === high - low
        return end || !group.some(member => shown.has(member)) ? [...group] : []
      }), selected)
      return
    }
    anchor.value = key
    apply([...(members(key) ?? [key])], selected)
  }

  function reset(): void {
    anchor.value = null
    extending.value = false
  }

  return { anchor, extending, noteModifier, pick, reset }
}
