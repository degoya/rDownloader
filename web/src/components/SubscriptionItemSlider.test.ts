import { fireEvent, screen } from '@testing-library/vue'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { nextTick } from 'vue'

import type { SubscriptionItem } from '@/api/types'
import linkgrabber from '@/locales/en/linkgrabber.json'
import subscriptions from '@/locales/en/subscriptions.json'
import { mountComponent, passthrough } from '@/test/mount'
import { CARD_AUTOPLAY_MS } from '@/utils/subscriptionHit'

import SubscriptionItemSlider from './SubscriptionItemSlider.vue'

/** jsdom has no layout, so the track is 0 px wide and the slider shows one card per page. */
function hit(id: string, title: string, attributes: Record<string, string> = {}): SubscriptionItem {
  return { id, title, state: 'pending', attributes } as SubscriptionItem
}

const hits = [
  hit('a', 'Alpha.Show.S01E01.1080p-GRP', { size: '1000000' }),
  hit('b', 'Beta.Show.S01E02.1080p-GRP', { resolution: '1080p' }),
  hit('c', 'Gamma.Show.S01E03.1080p-GRP')
]

function mount(props: Partial<{ items: SubscriptionItem[], autoplay: boolean, bulkBusy: boolean, total: number }> = {}) {
  return mountComponent(SubscriptionItemSlider, {
    props: { items: hits, label: 'HD TV', busyIds: [], bulkBusy: false, autoplay: false, ...props },
    messages: { linkgrabber, subscriptions },
    stubs: { ULink: passthrough }
  })
}

/** The name of the card on the current page. */
function shown(): string | null {
  return screen.getAllByTestId('subscription-card').map(card => card.getAttribute('aria-label')).join(',')
}

let reduce = false
let visibility: DocumentVisibilityState = 'visible'

beforeEach(() => {
  reduce = false
  visibility = 'visible'
  vi.stubGlobal('matchMedia', (query: string) => ({
    matches: query.includes('reduce') && reduce,
    addEventListener: () => {},
    removeEventListener: () => {}
  }))
  Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => visibility })
})

afterEach(() => {
  vi.useRealTimers()
  vi.unstubAllGlobals()
})

describe('SubscriptionItemSlider — paging', () => {
  it('turns pages with the arrows, wrapping at both ends', async () => {
    mount()
    expect(shown()).toBe('Alpha Show')
    await fireEvent.click(screen.getByRole('button', { name: 'Next page' }))
    expect(shown()).toBe('Beta Show')
    await fireEvent.click(screen.getByRole('button', { name: 'Previous page' }))
    await fireEvent.click(screen.getByRole('button', { name: 'Previous page' }))
    expect(shown()).toBe('Gamma Show')
  })

  it('turns pages with the arrow keys on the focusable track', async () => {
    mount()
    const track = screen.getByTestId('slider-track')
    expect(track.getAttribute('tabindex')).toBe('0')
    await fireEvent.keyDown(track, { key: 'ArrowRight' })
    expect(shown()).toBe('Beta Show')
    await fireEvent.keyDown(track, { key: 'ArrowLeft' })
    expect(shown()).toBe('Alpha Show')
  })

  it('turns pages with a swipe, and a tap is not a swipe', async () => {
    mount()
    const track = screen.getByTestId('slider-track')
    await fireEvent.pointerDown(track, { clientX: 300 })
    await fireEvent.pointerUp(track, { clientX: 290 })
    expect(shown()).toBe('Alpha Show')
    await fireEvent.pointerDown(track, { clientX: 300 })
    await fireEvent.pointerUp(track, { clientX: 150 })
    expect(shown()).toBe('Beta Show')
    await fireEvent.pointerDown(track, { clientX: 100 })
    await fireEvent.pointerUp(track, { clientX: 250 })
    expect(shown()).toBe('Alpha Show')
  })

  it('has operable dots that name their page and mark the current one', async () => {
    mount()
    const dots = screen.getAllByTestId('slider-dot')
    expect(dots).toHaveLength(3)
    expect(dots[0]?.getAttribute('aria-current')).toBe('true')
    await fireEvent.click(screen.getByRole('button', { name: 'Page 3 of 3' }))
    expect(shown()).toBe('Gamma Show')
    expect(screen.getAllByTestId('slider-dot')[2]?.getAttribute('aria-current')).toBe('true')
  })
})

/**
 * RD-130-13: the slider holds every hit of the subscription rather than one page under a
 * pagination bar, and reads the rest as the reader gets there. jsdom gives one card per page.
 */
describe('SubscriptionItemSlider — every hit, read as it is reached', () => {
  const read = Array.from({ length: 50 }, (_, index) => hit(`h${index}`, `Show.${index}.S01E01-GRP`))

  it('counts the pages of all the hits, not of the ones read so far', () => {
    mount({ items: read, total: 120 })
    // 120 pages are too many for dots; the slider says where it is instead.
    expect(screen.queryAllByTestId('slider-dot')).toHaveLength(0)
    expect(screen.getByTestId('slider-counter').textContent?.trim()).toBe('Page 1 of 120')
  })

  it('keeps its dots while there are few pages', () => {
    mount({ items: read.slice(0, 10), total: 10 })
    expect(screen.getAllByTestId('slider-dot')).toHaveLength(10)
    expect(screen.queryByTestId('slider-counter')).toBeNull()
  })

  it('asks for more when the next page reaches past what it has, and not before', async () => {
    const { emitted, rerender } = mount({ items: read.slice(0, 3), total: 120 })
    // Page 1 and the one after it are read: nothing to ask for yet.
    expect(emitted().more).toBeUndefined()
    await fireEvent.click(screen.getByRole('button', { name: 'Next page' }))
    await fireEvent.click(screen.getByRole('button', { name: 'Next page' }))
    expect(emitted().more).toHaveLength(1)
    expect(screen.getByTestId('slider-counter').textContent?.trim()).toBe('Page 3 of 120')

    // The answer arrives; the slider has what it needs and stays quiet.
    await rerender({ items: read.slice(0, 50), total: 120 })
    expect(emitted().more).toHaveLength(1)
  })

  it('holds a place for a card still being read rather than showing an empty page', async () => {
    mount({ items: read.slice(0, 1), total: 120 })
    await fireEvent.click(screen.getByRole('button', { name: 'Next page' }))
    expect(screen.queryAllByTestId('subscription-card')).toHaveLength(0)
    expect(screen.getAllByTestId('slider-pending')).toHaveLength(1)
  })

  it('renders one page of cards however long the archive is', () => {
    const many = Array.from({ length: 5000 }, (_, index) => hit(`m${index}`, `Show.${index}.S01E01-GRP`))
    const { container } = mount({ items: many, total: 5000 })
    // The cost of a long archive is the array, not the page: one card, no dots, one counter.
    expect(screen.getAllByTestId('subscription-card')).toHaveLength(1)
    expect(container.querySelectorAll('button[data-testid="slider-dot"]')).toHaveLength(0)
    expect(screen.getByTestId('slider-counter').textContent?.trim()).toBe('Page 1 of 5000')
  })

  it('never asks once everything is read', () => {
    const { emitted } = mount({ items: read.slice(0, 2), total: 2 })
    expect(emitted().more).toBeUndefined()
  })

  it('wraps from the first page to the last only once it holds every hit', async () => {
    mount({ items: read, total: 120 })
    const previous = screen.getByRole('button', { name: 'Previous page' }) as HTMLButtonElement
    // Wrapping here would mean reading all 120 before the page could be drawn.
    expect(previous.disabled).toBe(true)
    await fireEvent.keyDown(screen.getByTestId('slider-track'), { key: 'ArrowLeft' })
    expect(screen.getByTestId('slider-counter').textContent?.trim()).toBe('Page 1 of 120')
  })
})

describe('SubscriptionItemSlider — width', () => {
  async function mountAt(width: number): Promise<void> {
    const spy = vi.spyOn(HTMLElement.prototype, 'clientWidth', 'get').mockReturnValue(width)
    const many = Array.from({ length: 8 }, (_, index) => hit(`h${index}`, `Show.${index}.S01E01-GRP`))
    mount({ items: many })
    spy.mockRestore()
    await nextTick()
  }

  it('shows fewer cards per page in a narrow window rather than narrower cards', async () => {
    // 15rem cards with a 12 px gap: 1000 px holds four, 500 px two, 200 px still one.
    await mountAt(1000)
    expect(screen.getAllByTestId('subscription-card')).toHaveLength(4)
    expect(screen.getAllByTestId('slider-dot')).toHaveLength(2)
  })

  it('keeps at least one whole card in the narrowest window', async () => {
    await mountAt(200)
    expect(screen.getAllByTestId('subscription-card')).toHaveLength(1)
    expect(screen.getAllByTestId('slider-dot')).toHaveLength(8)
  })

  it('fits two cards at half that width', async () => {
    await mountAt(500)
    expect(screen.getAllByTestId('subscription-card')).toHaveLength(2)
  })
})

describe('SubscriptionItemSlider — actions', () => {
  it('queues and dismisses the card it was asked about', async () => {
    const { emitted } = mount()
    await fireEvent.click(screen.getByRole('button', { name: 'Queue' }))
    await fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }))
    expect(emitted().queue).toEqual([['a']])
    expect(emitted().dismiss).toEqual([['a']])
  })

  it('opens one details panel for the chosen card and closes it again', async () => {
    mount()
    expect(screen.queryByTestId('slider-details')).toBeNull()
    await fireEvent.click(screen.getByRole('button', { name: 'Details' }))
    expect(screen.getByTestId('slider-details').textContent).toContain('Alpha.Show.S01E01.1080p-GRP')
    await fireEvent.click(screen.getByRole('button', { name: 'Close details' }))
    expect(screen.queryByTestId('slider-details')).toBeNull()
  })

  it('locks the card actions while a bulk decision runs', () => {
    mount({ bulkBusy: true })
    expect((screen.getByRole('button', { name: 'Dismiss' }) as HTMLButtonElement).disabled).toBe(true)
  })
})

describe('SubscriptionItemSlider — autoplay', () => {
  async function tick(): Promise<void> {
    vi.advanceTimersByTime(CARD_AUTOPLAY_MS)
    await nextTick()
  }

  it('does not move unless the subscription asks for it', async () => {
    vi.useFakeTimers()
    mount()
    await tick()
    expect(shown()).toBe('Alpha Show')
    expect(screen.queryByTestId('slider-autoplay')).toBeNull()
  })

  it('turns a page every interval and wraps from the last page to the first', async () => {
    vi.useFakeTimers()
    mount({ autoplay: true })
    await tick()
    expect(shown()).toBe('Beta Show')
    await tick()
    expect(shown()).toBe('Gamma Show')
    await tick()
    expect(shown()).toBe('Alpha Show')
  })

  it('stops and resumes with its visible pause control', async () => {
    vi.useFakeTimers()
    mount({ autoplay: true })
    await fireEvent.click(screen.getByRole('button', { name: 'Pause autoplay' }))
    await tick()
    expect(shown()).toBe('Alpha Show')
    await fireEvent.click(screen.getByRole('button', { name: 'Resume autoplay' }))
    await tick()
    expect(shown()).toBe('Beta Show')
  })

  it('holds while the pointer is over the slider', async () => {
    vi.useFakeTimers()
    mount({ autoplay: true })
    const slider = screen.getByTestId('subscription-slider')
    await fireEvent.pointerEnter(slider)
    await tick()
    expect(shown()).toBe('Alpha Show')
    await fireEvent.pointerLeave(slider)
    await tick()
    expect(shown()).toBe('Beta Show')
  })

  it('holds while anything in it has keyboard focus', async () => {
    vi.useFakeTimers()
    mount({ autoplay: true })
    const track = screen.getByTestId('slider-track')
    await fireEvent.focusIn(track)
    await tick()
    expect(shown()).toBe('Alpha Show')
    await fireEvent.focusOut(track, { relatedTarget: document.body })
    await tick()
    expect(shown()).toBe('Beta Show')
  })

  it('holds while the details panel is open', async () => {
    vi.useFakeTimers()
    mount({ autoplay: true })
    await fireEvent.click(screen.getByRole('button', { name: 'Details' }))
    await tick()
    expect(shown()).toBe('Alpha Show')
    await fireEvent.click(screen.getByRole('button', { name: 'Close details' }))
    await tick()
    expect(shown()).toBe('Beta Show')
  })

  it('holds while the tab is hidden', async () => {
    vi.useFakeTimers()
    mount({ autoplay: true })
    visibility = 'hidden'
    document.dispatchEvent(new Event('visibilitychange'))
    await nextTick()
    await tick()
    expect(shown()).toBe('Alpha Show')
    visibility = 'visible'
    document.dispatchEvent(new Event('visibilitychange'))
    await nextTick()
    await tick()
    expect(shown()).toBe('Beta Show')
  })

  it('does not run at all, and offers no control, when reduced motion is asked for', async () => {
    reduce = true
    vi.useFakeTimers()
    mount({ autoplay: true })
    await tick()
    await tick()
    expect(shown()).toBe('Alpha Show')
    expect(screen.queryByTestId('slider-autoplay')).toBeNull()
  })
})
