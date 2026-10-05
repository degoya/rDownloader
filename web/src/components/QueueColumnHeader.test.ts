/**
 * The column header above the download list and the LinkGrabber (RD-191-11).
 *
 * Each case drives the header the way a viewer does — a key on the edge, a double click, a
 * drag — and reads the result where the rows read it: the custom property on the container
 * the view wraps around the header and the list.
 */
import { fireEvent, screen } from '@testing-library/vue'
import { beforeEach, describe, expect, it } from 'vitest'
import { defineComponent, h } from 'vue'

import type { SubscriptionItem } from '@/api/types'
import { QUEUE_COLUMN_DEFAULTS, QUEUE_COLUMN_LIMITS, queueColumnsStorageKey, useQueueColumns, type QueueColumnsView } from '@/composables/useQueueColumns'
import subscriptions from '@/locales/en/subscriptions.json'
import { mountComponent } from '@/test/mount'

import QueueColumnHeader from './QueueColumnHeader.vue'
import SubscriptionItemRow from './SubscriptionItemRow.vue'

/** What a view does: the composable's style on a container, the header inside it. */
const Harness = defineComponent({
  props: { view: { type: String as () => QueueColumnsView, default: 'downloads' } },
  setup(props) {
    const columns = useQueueColumns(props.view)
    return () => h('div', { 'data-testid': 'container', style: columns.style.value }, [
      h(QueueColumnHeader, {
        widths: columns.widths.value,
        view: props.view,
        customized: columns.customized.value,
        onResize: columns.setWidth,
        onReset: columns.reset,
        onResetAll: columns.resetAll
      })
    ])
  }
})

function renderHeader(view: QueueColumnsView = 'downloads') {
  return mountComponent(Harness, { props: { view } })
}

function container(): HTMLElement {
  return screen.getByTestId('container')
}

function edge(name: string): HTMLElement {
  return screen.getByRole('separator', { name: `Width of the “${name}” column` })
}

beforeEach(() => localStorage.clear())

describe('QueueColumnHeader', () => {
  it('labels the cells on the queue grid and gives every data column an edge', () => {
    renderHeader()
    const row = screen.getByTestId('queue-column-header').querySelector('.queue-row') as HTMLElement
    for (const cell of ['handle', 'select', 'expand', 'name', 'state', 'progress', 'size', 'meta', 'actions']) {
      expect(row.querySelector(`.queue-cell-${cell}`), cell).toBeTruthy()
    }
    expect(screen.getByText('Name')).toBeTruthy()
    expect(screen.getByText('State')).toBeTruthy()
    expect(screen.getByText('Progress')).toBeTruthy()
    expect(screen.getByText('Category · Account')).toBeTruthy()
    expect(screen.getAllByRole('separator')).toHaveLength(4)
  })

  // A LinkGrabber row puts a link's state, its size and its hoster or variant into the grid and
  // leaves the progress cell empty; the header said "Progress" and "State" over it (RD-1101-08).
  it('names the LinkGrabber\'s cells by what a link row shows, and leaves its empty one bare', () => {
    renderHeader('linkgrabber')
    const row = screen.getByTestId('queue-column-header').querySelector('.queue-row') as HTMLElement
    for (const cell of ['handle', 'select', 'expand', 'name', 'state', 'progress', 'size', 'meta', 'actions']) {
      expect(row.querySelector(`.queue-cell-${cell}`), cell).toBeTruthy()
    }
    const label = (cell: string) => row.querySelector(`.queue-cell-${cell}`)?.textContent?.trim()
    expect(label('name')).toBe('Name')
    expect(label('state')).toBe('Link state')
    expect(label('progress')).toBe('')
    expect(label('size')).toBe('Size')
    expect(label('meta')).toBe('Hoster · Variant')
    expect(row.textContent).not.toContain('Progress')
    expect(row.textContent).not.toContain('Category · Account')
    expect(screen.getAllByRole('separator').map(edge => edge.getAttribute('data-column'))).toEqual(['state', 'size', 'meta'])
  })

  it('keeps no width for the LinkGrabber\'s progress cell, not even a stored one', async () => {
    localStorage.setItem(queueColumnsStorageKey('linkgrabber'), JSON.stringify({ progress: 300, size: 200 }))
    renderHeader('linkgrabber')
    expect(container().style.getPropertyValue('--queue-col-progress')).toBe('')
    expect(container().style.getPropertyValue('--queue-col-size')).toBe('200px')
    await fireEvent.keyDown(edge('Size'), { key: 'Enter' })
    expect(localStorage.getItem(queueColumnsStorageKey('linkgrabber'))).toBeNull()
  })

  it('exposes each edge as a vertical separator with its width and limits', () => {
    renderHeader()
    const state = edge('State')
    expect(state.getAttribute('tabindex')).toBe('0')
    expect(state.getAttribute('aria-orientation')).toBe('vertical')
    expect(state.getAttribute('aria-valuenow')).toBe(String(QUEUE_COLUMN_DEFAULTS.state))
    expect(state.getAttribute('aria-valuemin')).toBe(String(QUEUE_COLUMN_LIMITS.state.min))
    expect(state.getAttribute('aria-valuemax')).toBe(String(QUEUE_COLUMN_LIMITS.state.max))
  })

  it('moves an edge with the arrow keys and sets the custom property on the container', async () => {
    renderHeader()
    const meta = edge('Category · Account')
    await fireEvent.keyDown(meta, { key: 'ArrowLeft' })
    expect(container().style.getPropertyValue('--queue-col-meta')).toBe('184px')
    await fireEvent.keyDown(meta, { key: 'ArrowRight', shiftKey: true })
    expect(container().style.getPropertyValue('--queue-col-meta')).toBe('152px')
    expect(meta.getAttribute('aria-valuenow')).toBe('152')
    expect(JSON.parse(localStorage.getItem(queueColumnsStorageKey('downloads')) ?? 'null')).toEqual({ meta: 152 })
  })

  it('stops at the column limit however often the key is pressed', async () => {
    renderHeader()
    const progress = edge('Progress')
    for (let press = 0; press < 10; press += 1) await fireEvent.keyDown(progress, { key: 'ArrowRight', shiftKey: true })
    expect(container().style.getPropertyValue('--queue-col-progress')).toBe(`${QUEUE_COLUMN_LIMITS.progress.min}px`)
  })

  it('puts a column back with Enter and with a double click', async () => {
    renderHeader()
    const size = edge('Size')
    await fireEvent.keyDown(size, { key: 'ArrowLeft', shiftKey: true })
    expect(container().style.getPropertyValue('--queue-col-size')).toBe('176px')
    await fireEvent.keyDown(size, { key: 'Enter' })
    expect(container().style.getPropertyValue('--queue-col-size')).toBe('144px')

    await fireEvent.keyDown(size, { key: 'ArrowLeft' })
    expect(container().style.getPropertyValue('--queue-col-size')).toBe('152px')
    await fireEvent.dblClick(size)
    expect(container().style.getPropertyValue('--queue-col-size')).toBe('144px')
  })

  it('follows a pointer drag: to the left widens, to the right narrows', async () => {
    renderHeader()
    const state = edge('State')
    await fireEvent.pointerDown(state, { pointerId: 1, button: 0, clientX: 500 })
    await fireEvent.pointerMove(state, { pointerId: 1, clientX: 460 })
    expect(container().style.getPropertyValue('--queue-col-state')).toBe('168px')
    await fireEvent.pointerMove(state, { pointerId: 1, clientX: 520 })
    expect(container().style.getPropertyValue('--queue-col-state')).toBe('108px')
    await fireEvent.pointerUp(state, { pointerId: 1, clientX: 520 })
    await fireEvent.pointerMove(state, { pointerId: 1, clientX: 300 })
    expect(container().style.getPropertyValue('--queue-col-state')).toBe('108px')
  })

  it('resets every column from the header menu, offered only once something changed', async () => {
    renderHeader()
    const resetAll = screen.getByRole('button', { name: 'Reset all column widths' }) as HTMLButtonElement
    expect(resetAll.disabled).toBe(true)
    await fireEvent.keyDown(edge('State'), { key: 'ArrowLeft' })
    await fireEvent.keyDown(edge('Size'), { key: 'ArrowLeft' })
    expect(resetAll.disabled).toBe(false)
    await fireEvent.click(resetAll)
    expect(container().style.getPropertyValue('--queue-col-state')).toBe('128px')
    expect(container().style.getPropertyValue('--queue-col-size')).toBe('144px')
    expect(localStorage.getItem(queueColumnsStorageKey('downloads'))).toBeNull()
  })
})

describe('SubscriptionItemRow', () => {
  // Deliberately not on the queue grid (RD-110-27); the adjustable widths must not reach it.
  it('stays off the queue grid and its column properties', () => {
    mountComponent(SubscriptionItemRow, {
      messages: { subscriptions },
      props: { item: { id: 'hit-1', title: 'Some.Release.1080p', state: 'pending', attributes: {} } as unknown as SubscriptionItem }
    })
    expect(document.querySelector('.queue-row')).toBeNull()
    expect(document.querySelector('[class*="queue-cell"]')).toBeNull()
    expect(document.querySelector('[style*="--queue-col"]')).toBeNull()
  })
})
