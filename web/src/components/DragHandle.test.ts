/**
 * The one drag handle every reorderable list uses (RD-1120-14, `design.md`, *A drag starts at the
 * handle, and only there*): a named, focusable button that starts the drag and answers the arrow
 * keys, its keyboard equivalent.
 */
import { fireEvent, screen } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'

import { mountComponent } from '@/test/mount'

import DragHandle from './DragHandle.vue'

function renderHandle() {
  return mountComponent(DragHandle, { props: { label: 'Drag to reorder — arrow keys move it' } })
}

describe('DragHandle', () => {
  it('is a draggable button named by its label', () => {
    renderHandle()
    const handle = screen.getByRole('button', { name: 'Drag to reorder — arrow keys move it' })
    expect(handle.getAttribute('draggable')).toBe('true')
    expect(handle.getAttribute('title')).toBe('Drag to reorder — arrow keys move it')
    expect(handle.hasAttribute('data-row-handle')).toBe(true)
  })

  it('says when a drag starts at it', async () => {
    const { emitted } = renderHandle()
    await fireEvent.dragStart(screen.getByRole('button'))
    expect(emitted().dragstart).toHaveLength(1)
  })

  it('moves one step per arrow key', async () => {
    const { emitted } = renderHandle()
    const handle = screen.getByRole('button')
    await fireEvent.keyDown(handle, { key: 'ArrowUp' })
    await fireEvent.keyDown(handle, { key: 'ArrowDown' })
    expect(emitted().move).toEqual([[-1], [1]])
  })
})
