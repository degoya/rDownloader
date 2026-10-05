/**
 * One indexer subscription's waiting hits: the bulk actions sit in the header and repeat in a
 * footer under an open list, so a reader at the end of a long list need not scroll back up.
 */
import { fireEvent, screen, within } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'

import linkgrabberCatalogue from '@/locales/en/linkgrabber.json'
import { mountComponent } from '@/test/mount'

import IndexerReviewGroup from './IndexerReviewGroup.vue'

function props(overrides: Record<string, unknown> = {}) {
  return {
    subscription: { id: 'sub-1', name: 'omgwtfnzbs HD', view: 'list' },
    items: [{ id: 'item-1', title: 'Release.One.1080p', link: 'https://indexer.example/1' }],
    total: 1,
    listTotal: 1,
    page: 1,
    pageSize: 50,
    loading: false,
    error: null,
    busyIds: [],
    bulkBusy: false,
    ...overrides
  }
}

function mount(overrides: Record<string, unknown> = {}) {
  return mountComponent(IndexerReviewGroup, {
    props: props(overrides),
    messages: { linkgrabber: linkgrabberCatalogue },
    stubs: {
      SubscriptionItemRow: { template: '<li><slot name="actions" /></li>' },
      SubscriptionItemActions: { template: '<span />' },
      SubscriptionItemSlider: { template: '<div data-testid="slider" />' },
      DataState: { template: '<div />' },
      UPagination: { template: '<nav aria-label="Pages" />' }
    }
  })
}

async function openGroup(): Promise<void> {
  await fireEvent.click(screen.getByRole('button', { name: 'omgwtfnzbs HD' }))
}

describe('IndexerReviewGroup', () => {
  it('repeats the bulk actions in a footer under the open list', async () => {
    const { emitted } = mount()
    expect(screen.queryByTestId('subscription-group-footer')).toBeNull()
    await openGroup()
    const footer = screen.getByTestId('subscription-group-footer')
    await fireEvent.click(within(footer).getByText(linkgrabberCatalogue.indexers.queue_all))
    await fireEvent.click(within(footer).getByText(linkgrabberCatalogue.indexers.dismiss_all))
    expect(emitted('queueAll')).toHaveLength(1)
    expect(emitted('dismissAll')).toHaveLength(1)
  })

  // Owner, 2026-10-05 (RD-1101-01): the actions belong at the end of a group that fits on one
  // page as much as of one with pages; the footer must not hang on the pagination bar.
  it('repeats the bulk actions under a list of one page, without pages', async () => {
    mount()
    await openGroup()
    const footer = screen.getByTestId('subscription-group-footer')
    expect(within(footer).queryByRole('navigation')).toBeNull()
    expect(within(footer).getByText(linkgrabberCatalogue.indexers.queue_all)).toBeTruthy()
    expect(within(footer).getByText(linkgrabberCatalogue.indexers.dismiss_all)).toBeTruthy()
  })

  it('puts the bulk actions beside the pages when the list has several', async () => {
    const { emitted } = mount({ total: 120 })
    await openGroup()
    const footer = screen.getByTestId('subscription-group-footer')
    expect(within(footer).getByRole('navigation')).toBeTruthy()
    await fireEvent.click(within(footer).getByText(linkgrabberCatalogue.indexers.dismiss_all))
    expect(emitted('dismissAll')).toHaveLength(1)
  })

  it('locks the bulk actions in header and footer alike while one runs', async () => {
    mount({ bulkBusy: true })
    await openGroup()
    const dismiss = screen.getAllByRole('button', { name: linkgrabberCatalogue.indexers.dismiss_all })
    expect(dismiss).toHaveLength(2)
    for (const button of dismiss) expect((button as HTMLButtonElement).disabled).toBe(true)
  })

  it('has no footer for an empty list', async () => {
    mount({ items: [] })
    await openGroup()
    expect(screen.queryByTestId('subscription-group-footer')).toBeNull()
  })

  // Owner, 2026-10-04: the actions belong at the end of the group in the cards view as well;
  // only the pages stay out, because the slider loads more by itself.
  it('repeats the bulk actions under the cards view, without pages', async () => {
    const { emitted } = mount({
      subscription: { id: 'sub-1', name: 'omgwtfnzbs HD', view: 'cards' },
      total: 500
    })
    await openGroup()
    const footer = screen.getByTestId('subscription-group-footer')
    expect(within(footer).queryByRole('navigation')).toBeNull()
    await fireEvent.click(within(footer).getByText(linkgrabberCatalogue.indexers.queue_all))
    expect(emitted('queueAll')).toHaveLength(1)
  })
})
