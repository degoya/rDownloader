import { fireEvent } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'

import common from '@/locales/en/common.json'
import downloads from '@/locales/en/downloads.json'
import { axeViolations } from '@/test/axe'
import { mountComponent } from '@/test/mount'

import BulkActionBar from './BulkActionBar.vue'

const SearchableSelect = { props: ['modelValue', 'items'], template: '<select v-bind="$attrs"></select>' }

function mount(props: Record<string, unknown> = {}) {
  return mountComponent(BulkActionBar, {
    messages: { downloads },
    props: { count: 3, categories: [], unitKey: 'common.units.file', transferActions: true, ...props },
    stubs: { SearchableSelect }
  })
}

/** The selection bar of the queue, compact enough for one line (RD-1220-03). */
describe('BulkActionBar', () => {
  it('shows start, pause, stop and remove as icons that still have a name', () => {
    const { getByRole } = mount()
    for (const name of [common.actions.start, common.actions.pause, common.actions.cancel, common.actions.remove]) {
      const button = getByRole('button', { name })
      // Named by `aria-label` and `title`, with no visible text of its own.
      expect(button.getAttribute('title')).toBe(name)
      expect(button.textContent?.trim()).toBe('')
    }
  })

  it('keeps the label of an action that is not self-evident', () => {
    const { getByRole } = mount()
    expect(getByRole('button', { name: common.actions.extract }).textContent).toContain(common.actions.extract)
  })

  it('puts the close button outside the group that wraps, so it stays in the window', () => {
    const { getByTestId } = mount()
    const bar = getByTestId('bulk-action-bar')
    const group = getByTestId('bulk-action-group')
    const clear = getByTestId('bulk-clear')
    // jsdom lays nothing out; the structure is what holds the X in: the group wraps and may
    // shrink below its content, the X is the bar's own last child and does not shrink.
    expect(clear.parentElement).toBe(bar)
    expect(bar.lastElementChild).toBe(clear)
    expect(group.className).toContain('flex-wrap')
    expect(group.className).toContain('min-w-0')
    expect(group.className).toContain('flex-1')
    expect(clear.className).toContain('shrink-0')
    expect(clear.getAttribute('aria-label')).toBe(common.actions.clear_selection)
  })

  it('keeps the order of the keyboard: the group first, the X last', () => {
    const { getAllByRole } = mount()
    const names = getAllByRole('button').map(button => button.getAttribute('aria-label'))
    expect(names.at(-2)).toBe(common.actions.remove)
    expect(names.at(-1)).toBe(common.actions.clear_selection)
  })

  it('emits what each icon stands for', async () => {
    const { getByRole, emitted } = mount()
    await fireEvent.click(getByRole('button', { name: common.actions.start }))
    await fireEvent.click(getByRole('button', { name: common.actions.cancel }))
    await fireEvent.click(getByRole('button', { name: common.actions.remove }))
    await fireEvent.click(getByRole('button', { name: common.actions.clear_selection }))
    expect(Object.keys(emitted())).toEqual(expect.arrayContaining(['resume', 'cancel', 'remove', 'clear']))
  })

  it('keeps the LinkGrabber pair labelled', () => {
    const { getByRole } = mount({ transferActions: false })
    expect(getByRole('button', { name: downloads.bulk.enqueue }).textContent).toContain(downloads.bulk.enqueue)
    expect(getByRole('button', { name: common.actions.remove }).textContent?.trim()).toBe('')
  })

  it('has no accessibility violations', async () => {
    const { container } = mount()
    expect(await axeViolations(container)).toBe('')
  })
})
