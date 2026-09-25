/**
 * The shared long-list block (RD-106-12).
 *
 * The queue and the LinkGrabber both hand it a flat stream of rows; what is checked here is the
 * block itself — when it starts windowing, what it keeps in the document anyway, and whether a
 * row nobody has scrolled to can still be reached and focused.
 */
import { fireEvent, render } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'
import { defineComponent, h, nextTick, ref } from 'vue'

import type { VirtualRow } from '@/composables/useVirtualRows'

import VirtualRowList from './VirtualRowList.vue'

interface Row extends VirtualRow { label: string }

function rows(count: number): Row[] {
  return Array.from({ length: count }, (_, index) => ({ key: `row-${index}`, size: 40, label: `Row ${index}` }))
}

/** A parent, so the exposed `focusRow` / `revealRow` are reachable the way a view reaches them. */
function harness(count: number, pinnedKeys: string[] = []) {
  const list = ref<{ focusRow: (key: string) => Promise<boolean>, revealRow: (key: string) => Promise<boolean> } | null>(null)
  const Harness = defineComponent({
    setup() {
      return () => h(VirtualRowList as never, {
        ref: list,
        rows: rows(count),
        label: `Rows, ${count} of them`,
        pinnedKeys
      }, {
        row: ({ row }: { row: Row }) => h('div', {}, [
          h('button', { type: 'button', 'data-row-handle': '' }, 'handle'),
          row.label
        ])
      })
    }
  })
  return { list, ...render(Harness) }
}

function keysIn(container: Element): string[] {
  return [...container.querySelectorAll('[data-row-key]')].map(node => node.getAttribute('data-row-key') ?? '')
}

describe('VirtualRowList', () => {
  /** A short list is not worth windowing, and a list that does not window cannot go wrong. */
  it('renders a short list whole and gives it no scroll viewport of its own', () => {
    const { container } = harness(20)
    expect(keysIn(container)).toHaveLength(20)
    expect(container.firstElementChild?.className).not.toContain('overflow-y-auto')
  })

  it('windows a long list and says how long it really is', () => {
    const { container } = harness(500)
    const rendered = keysIn(container)
    expect(rendered.length).toBeLessThan(60)
    expect(rendered[0]).toBe('row-0')

    const list = container.querySelector('[role="list"]')
    expect(list?.getAttribute('aria-label')).toBe('Rows, 500 of them')
    const first = container.querySelector('[role="listitem"]')
    expect(first?.getAttribute('aria-setsize')).toBe('500')
    expect(first?.getAttribute('aria-posinset')).toBe('1')
  })

  /**
   * The padding stands in for what is not rendered, so the scrollbar means what it says. It is
   * padding rather than spacer elements because a `role="list"` may only hold list items.
   */
  it('pays for the rows it left out with padding, so the total height is right', async () => {
    const { container } = harness(500)
    const viewport = container.firstElementChild as HTMLElement
    viewport.scrollTop = 4000
    await fireEvent.scroll(viewport)
    await nextTick()

    const list = container.querySelector('[role="list"]') as HTMLElement
    const padTop = Number.parseFloat(list.style.paddingTop)
    const padBottom = Number.parseFloat(list.style.paddingBottom)
    const renderedHeight = keysIn(container).length * 40
    expect(padTop).toBeGreaterThan(0)
    expect(padTop + padBottom + renderedHeight).toBe(500 * 40)
  })

  /** A row the caller pins stays in the document wherever the window happens to be. */
  it('keeps a pinned row rendered even when it is far outside the window', async () => {
    const { container } = harness(500, ['row-400'])
    expect(keysIn(container)).toContain('row-400')

    const viewport = container.firstElementChild as HTMLElement
    viewport.scrollTop = 0
    await fireEvent.scroll(viewport)
    await nextTick()
    expect(keysIn(container)).toContain('row-400')
    expect(keysIn(container)).not.toContain('row-399')
  })

  /**
   * The jump to an entry, and what a keyboard reorder needs when the row it moved has left the
   * window: the row is rendered first and the handle inside it takes the focus.
   */
  it('reaches a row that is not rendered and puts the keyboard on its handle', async () => {
    const { container, list } = harness(500)
    expect(keysIn(container)).not.toContain('row-300')

    expect(await list.value?.focusRow('row-300')).toBe(true)
    await nextTick()

    expect(keysIn(container)).toContain('row-300')
    const focused = (document.activeElement as HTMLElement | null)?.closest('[data-row-key]')
    expect(focused?.getAttribute('data-row-key')).toBe('row-300')
  })

  it('reports a key it does not hold rather than pretending it jumped', async () => {
    const { list } = harness(500)
    expect(await list.value?.focusRow('row-9999')).toBe(false)
    expect(await list.value?.revealRow('row-9999')).toBe(false)
  })

  /** Focus alone is enough to pin: nothing has to be told which row the keyboard is on. */
  it('pins whatever row has focus, without being asked', async () => {
    const { container } = harness(500)
    const handle = container.querySelector<HTMLElement>('[data-row-key="row-2"] [data-row-handle]')
    handle?.focus()

    const viewport = container.firstElementChild as HTMLElement
    viewport.scrollTop = 8000
    await fireEvent.scroll(viewport)
    await nextTick()

    expect(keysIn(container)).toContain('row-2')
    expect(keysIn(container)).not.toContain('row-3')
    expect(document.activeElement).toBe(handle)
  })
})
