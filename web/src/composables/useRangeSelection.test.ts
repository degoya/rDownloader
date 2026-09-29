import { describe, expect, it } from 'vitest'
import { nextTick, ref } from 'vue'

import { useRangeSelection } from './useRangeSelection'

/** A flat list `a`..`f`, or whatever order a case hands in, with the selection as a set. */
function setup(keys = ['a', 'b', 'c', 'd', 'e', 'f'], members: (key: string) => readonly string[] | undefined = () => undefined) {
  const order = ref(keys)
  const selected = ref(new Set<string>())
  const range = useRangeSelection(order, (picked, value) => {
    const next = new Set(selected.value)
    for (const key of picked) value ? next.add(key) : next.delete(key)
    selected.value = next
  }, members)
  const chosen = () => [...selected.value].sort()
  return { order, range, chosen }
}

function click(init: MouseEventInit = {}): MouseEvent {
  return new MouseEvent('click', { detail: 1, ...init })
}

describe('useRangeSelection (RD-170-13)', () => {
  it('selects from the anchor forwards to the shift-clicked row', () => {
    const { range, chosen } = setup()
    range.pick('b', true)
    range.noteModifier(click({ shiftKey: true }))
    range.pick('e', true)
    expect(chosen()).toEqual(['b', 'c', 'd', 'e'])
  })

  it('selects backwards just as well', () => {
    const { range, chosen } = setup()
    range.pick('e', true)
    range.noteModifier(click({ shiftKey: true }))
    range.pick('b', true)
    expect(chosen()).toEqual(['b', 'c', 'd', 'e'])
  })

  /** Every row in the range takes the state the clicked row has after the click. */
  it('clears a range when the clicked row was cleared', () => {
    const { range, chosen } = setup()
    range.pick('a', true)
    range.pick('f', true, true)
    range.pick('e', false, true)
    expect(chosen()).toEqual(['f'])
  })

  it('keeps the anchor while the range is stretched', () => {
    const { range, chosen } = setup()
    range.pick('b', true)
    range.pick('e', true, true)
    range.pick('c', true, true)
    expect(range.anchor.value).toBe('b')
    expect(chosen()).toEqual(['b', 'c', 'd', 'e'])
  })

  /** Ctrl/Cmd keeps today's toggle: one row, and it becomes the next anchor. */
  it('toggles a single row with ctrl and moves the anchor there', () => {
    const { range, chosen } = setup()
    range.pick('b', true)
    range.noteModifier(click({ ctrlKey: true }))
    range.pick('d', true)
    expect(chosen()).toEqual(['b', 'd'])
    expect(range.anchor.value).toBe('d')
    range.noteModifier(click({ metaKey: true }))
    range.pick('b', false)
    expect(chosen()).toEqual(['d'])
  })

  it('picks the one row when shift has no anchor to reach from', () => {
    const { range, chosen } = setup()
    range.pick('c', true, true)
    expect(chosen()).toEqual(['c'])
  })

  /** Shift+Space: the keydown says shift, the click the keyboard produces carries `detail` 0. */
  it('treats shift+space on a focused checkbox like a shift-click', () => {
    const { range, chosen } = setup()
    range.pick('b', true)
    range.noteModifier(new KeyboardEvent('keydown', { key: ' ', shiftKey: true }))
    range.noteModifier(new MouseEvent('click', { detail: 0 }))
    range.pick('d', true)
    expect(chosen()).toEqual(['b', 'c', 'd'])
  })

  it('does not carry shift over to the next plain click', () => {
    const { range, chosen } = setup()
    range.pick('a', true)
    range.noteModifier(click({ shiftKey: true }))
    range.pick('c', true)
    range.noteModifier(click())
    range.pick('f', true)
    expect(chosen()).toEqual(['a', 'b', 'c', 'f'])
  })

  /** A stale anchor must never pick rows: once its row is gone, shift has nothing to reach from. */
  it('drops the anchor when its row leaves the list', async () => {
    const { order, range, chosen } = setup()
    range.pick('b', true)
    order.value = ['a', 'c', 'd', 'e', 'f']
    await nextTick()
    expect(range.anchor.value).toBeNull()
    range.pick('e', true, true)
    expect(chosen()).toEqual(['b', 'e'])
  })

  /** A re-sort keeps the anchor row, and the range follows the order now on screen. */
  it('follows the new order when the rows are re-sorted', async () => {
    const { order, range, chosen } = setup()
    range.pick('b', true)
    order.value = ['f', 'e', 'd', 'c', 'b', 'a']
    await nextTick()
    range.pick('d', true, true)
    expect(chosen()).toEqual(['b', 'c', 'd'])
  })

  it('forgets the anchor on reset', () => {
    const { range } = setup()
    range.pick('b', true)
    range.reset()
    expect(range.anchor.value).toBeNull()
  })

  describe('with group rows', () => {
    // p1 is open (its files are on screen), p2 is collapsed, p3 is open.
    const members: Record<string, string[]> = { p1: ['a', 'b'], p2: ['c', 'd'], p3: ['e', 'f'] }
    const groups = () => setup(['p1', 'a', 'b', 'p2', 'p3', 'e', 'f'], key => members[key])

    it('brings a group row\'s members along on a plain click', () => {
      const { range, chosen } = groups()
      range.pick('p2', true)
      expect(chosen()).toEqual(['c', 'd'])
    })

    it('takes a collapsed group inside the range whole, and an open one only by its rows', () => {
      const { range, chosen } = groups()
      range.pick('b', true)
      range.pick('e', true, true)
      expect(chosen()).toEqual(['b', 'c', 'd', 'e'])
    })

    it('takes a group at either end of the range whole', () => {
      const { range, chosen } = groups()
      range.pick('p3', true)
      range.pick('p1', true, true)
      expect(chosen()).toEqual(['a', 'b', 'c', 'd', 'e', 'f'])
    })
  })
})
