import { fireEvent, render } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'

import common from '@/locales/en/common.json'
import downloads from '@/locales/en/downloads.json'
import { axeViolations } from '@/test/axe'
import { createTestI18n, mountComponent, uiStubs } from '@/test/mount'
import { SHARED_BULK_ACTIONS } from '@/utils/listLayout'

import BulkActionBar from './BulkActionBar.vue'

const SearchableSelect = { props: ['modelValue', 'items'], template: '<select v-bind="$attrs"></select>' }

function mount(props: Record<string, unknown> = {}) {
  return mountComponent(BulkActionBar, {
    messages: { downloads },
    props: { count: 3, categories: [], detail: '3 files in 2 packages', transferActions: true, ...props },
    stubs: { SearchableSelect }
  })
}

/** The selection bar of both lists, one line of icons (RD-1220-03, RD-1230-02). */
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

  it('shows every action as an icon with a name, extract and show-in-list included', () => {
    const { getByTestId } = mount()
    const buttons = [...getByTestId('bulk-action-bar').querySelectorAll('button')]
    expect(buttons.length).toBeGreaterThan(5)
    for (const button of buttons) {
      expect(button.getAttribute('aria-label')).toBeTruthy()
      expect(button.getAttribute('title')).toBeTruthy()
      expect(button.textContent?.trim()).toBe('')
    }
    expect(getByTestId('bulk-action-bar').querySelector('[aria-label="' + common.actions.reveal + '"]')).not.toBeNull()
  })

  it('says "N selected" on the badge, with what is selected in its tooltip', () => {
    const { getByText } = mount()
    const badge = getByText('3 selected')
    expect(badge.getAttribute('title')).toBe('3 files in 2 packages')
  })

  it('shows a red action with a count as its icon and figure, the sentence as its name', () => {
    const danger = { template: '<button type="button" aria-label="Reset 2 files" title="Reset 2 files">2</button>' }
    const { getByRole } = render(BulkActionBar, {
      props: { count: 3, categories: [], transferActions: true },
      slots: { danger },
      global: { plugins: [createTestI18n({ downloads })], stubs: { ...uiStubs, SearchableSelect } }
    })
    const reset = getByRole('button', { name: 'Reset 2 files' })
    expect(reset.textContent?.trim()).toBe('2')
    // Before remove: the view's own red actions stand at the end, remove last of all.
    expect(reset.nextElementSibling?.getAttribute('aria-label')).toBe(common.actions.remove)
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

  it('orders the shared actions the same way in the LinkGrabber, its enqueue pair as icons', async () => {
    const transfer = mount()
    const order = (root: Element) => [...root.querySelectorAll<HTMLElement>('[data-bulk-action]')].map(button => button.dataset.bulkAction)
    expect(order(transfer.container)).toEqual([...SHARED_BULK_ACTIONS])
    transfer.unmount()
    const { container, getByRole, emitted } = mount({ transferActions: false })
    expect(order(container)).toEqual([...SHARED_BULK_ACTIONS])
    expect(getByRole('button', { name: downloads.bulk.enqueue }).textContent?.trim()).toBe('')
    await fireEvent.click(getByRole('button', { name: downloads.bulk.enqueue_paused }))
    await fireEvent.click(getByRole('button', { name: common.actions.reveal }))
    await fireEvent.click(getByRole('button', { name: common.export.action }))
    expect(Object.keys(emitted())).toEqual(expect.arrayContaining(['enqueuePaused', 'reveal', 'export']))
  })

  it('has no accessibility violations', async () => {
    const { container } = mount()
    expect(await axeViolations(container)).toBe('')
  })
})
