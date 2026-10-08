/**
 * The notice over a download list sorted for the eye (RD-1190-16): what it is sorted by, that
 * the queue runs unchanged and dragging is off, and the way back.
 */
import { fireEvent, screen } from '@testing-library/vue'
import { describe, expect, it, vi } from 'vitest'

import downloads from '@/locales/en/downloads.json'
import { axeViolations } from '@/test/axe'
import { mountComponent } from '@/test/mount'

import QueueSortNotice from './QueueSortNotice.vue'

vi.mock('@nuxt/ui/components/Icon.vue', () => ({ default: { template: '<span />' } }))

describe('QueueSortNotice', () => {
  it('says the view is sorted, by what, and that the queue is unchanged', () => {
    mountComponent(QueueSortNotice, { props: { sort: { column: 'meta', direction: 'desc' } }, messages: { downloads } })
    expect(screen.getByText('View sorted – queue unchanged')).toBeTruthy()
    expect(screen.getByText(/By “Category”, descending\. .*dragging to reorder is off/)).toBeTruthy()
  })

  it('goes back to the queue order', async () => {
    const onReset = vi.fn()
    mountComponent(QueueSortNotice, { props: { sort: { column: 'size', direction: 'asc' }, onReset }, messages: { downloads } })
    await fireEvent.click(screen.getByRole('button', { name: 'Back to queue order' }))
    expect(onReset).toHaveBeenCalledTimes(1)
  })

  it('has no accessibility violations', async () => {
    const view = mountComponent(QueueSortNotice, { props: { sort: { column: 'name', direction: 'asc' } }, messages: { downloads } })
    expect(await axeViolations(view.container)).toBe('')
  })
})
