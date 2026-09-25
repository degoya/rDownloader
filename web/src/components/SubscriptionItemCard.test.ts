import { fireEvent, screen } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'

import type { SubscriptionItem } from '@/api/types'
import linkgrabber from '@/locales/en/linkgrabber.json'
import subscriptions from '@/locales/en/subscriptions.json'
import { mountComponent } from '@/test/mount'
import { CARD_RATIOS, type CardRatio, hitColour } from '@/utils/subscriptionHit'

import SubscriptionItemCard from './SubscriptionItemCard.vue'

function item(title: string, attributes: Record<string, string> = {}, extra: Partial<SubscriptionItem> = {}): SubscriptionItem {
  return { id: 'i1', title, state: 'pending', attributes, ...extra } as SubscriptionItem
}

function mount(props: { item: SubscriptionItem, showImages?: boolean, selected?: boolean, ratio?: CardRatio }) {
  return mountComponent(SubscriptionItemCard, {
    props: { selected: false, busy: false, ...props },
    messages: { linkgrabber, subscriptions }
  })
}

const full = item('The.Big.Bang.Theory.2007.S11E23.German.1080p.WEB-FUZEER', {
  tvtitle: 'The Big Bang Theory',
  season: '11',
  episode: '23',
  size: '737148928',
  password: '1',
  imdbscore: '8.1',
  language: 'Multi',
  resolution: '1080p',
  video: 'H.265',
  grabs: '4',
  coverurl: 'https://img.example.test/bbt.jpg'
})

describe('SubscriptionItemCard', () => {
  it('shows what the indexer sent: cover, size with lock, group, episode, release name, chips', () => {
    mount({ item: full })
    expect(screen.getByTestId('card-cover').getAttribute('src')).toBe('https://img.example.test/bbt.jpg')
    expect(screen.getByText('FUZEER')).toBeTruthy()
    expect(screen.getByText('S11E23')).toBeTruthy()
    expect(screen.getByText('The.Big.Bang.Theory.2007.S11E23.German.1080p.WEB-FUZEER')).toBeTruthy()
    for (const chip of ['IMDb 8.1', 'Multi', '1080p', 'H.265', 'Grabs 4']) expect(screen.getByText(chip)).toBeTruthy()
    expect(screen.getByText(/703/)).toBeTruthy()
  })

  it('heads the card with the release name and does not repeat its head above it (RD-130-13)', () => {
    mount({ item: full })
    // The short name was a line of its own over the release name that already begins with it.
    // It still names the card for a screen reader, and nowhere on it as text.
    const release = screen.getByTestId('card-release')
    expect(release.textContent).toBe('The.Big.Bang.Theory.2007.S11E23.German.1080p.WEB-FUZEER')
    expect(release.className).toContain('font-semibold')
    expect(screen.getByTestId('card-body').firstElementChild).toBe(release)
    expect(screen.queryByText('The Big Bang Theory')).toBeNull()
    expect(screen.getByTestId('subscription-card').getAttribute('aria-label')).toBe('The Big Bang Theory')
  })

  it('draws the initials tile instead of the cover when external pictures are off', () => {
    mount({ item: full, showImages: false })
    expect(screen.queryByTestId('card-cover')).toBeNull()
    expect(screen.getByTestId('card-initials').textContent).toBe('BBT')
    expect(screen.getByTestId('card-tile').getAttribute('style')).toContain(
      // jsdom normalises the colour to rgb(); compare against the same derivation.
      toRgb(hitColour('The Big Bang Theory'))
    )
  })

  it('leaves out what the indexer did not send, and keeps its height doing so', () => {
    mount({ item: item('Plain title') })
    expect(screen.queryByTestId('card-group')).toBeNull()
    expect(screen.queryByText(/iB/)).toBeNull()
    expect(screen.queryByRole('listitem')).toBeNull()
    // The picture follows the ratio and the text below has one fixed height, so a bare card and
    // a full one of the same width line up in the slider (RD-120-42).
    expect(screen.getByTestId('card-body').className).toContain('h-48')
    expect(screen.getByTestId('card-tile').className).not.toMatch(/\bh-\d/)
  })

  it('draws the picture area at 2:1 unless the subscription chose another ratio (RD-120-42)', () => {
    mount({ item: full })
    expect(screen.getByTestId('card-tile').dataset.ratio).toBe('2:1')
  })

  it.each(CARD_RATIOS)('gives the picture area the chosen ratio %s, cover and initials alike (RD-120-42)', (ratio) => {
    const { unmount } = mount({ item: full, ratio })
    const tile = screen.getByTestId('card-tile')
    expect(tile.dataset.ratio).toBe(ratio)
    expect(tile.style.getPropertyValue('aspect-ratio') || tile.getAttribute('style')).toContain(ratio.replace(':', ' / '))
    // The cover fills and crops the area rather than stretching it or setting its height.
    expect(screen.getByTestId('card-cover').className).toContain('object-cover')
    expect(screen.getByTestId('card-cover').className).toContain('absolute')
    unmount()

    mount({ item: full, ratio, showImages: false })
    expect(screen.getByTestId('card-tile').dataset.ratio).toBe(ratio)
    expect(screen.getByTestId('card-initials')).toBeTruthy()
  })

  it('shows a symbol by category when the title has nothing to take initials from', () => {
    mount({ item: item('—', {}, { source_category: '3010' }), showImages: false })
    expect(screen.queryByTestId('card-initials')).toBeNull()
    expect(screen.getByTestId('card-symbol')).toBeTruthy()
  })

  it('offers queue, dismiss and details, each with a name', async () => {
    const { emitted } = mount({ item: full })
    await fireEvent.click(screen.getByRole('button', { name: 'Queue' }))
    await fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }))
    await fireEvent.click(screen.getByRole('button', { name: 'Details' }))
    expect(emitted().queue).toHaveLength(1)
    expect(emitted().dismiss).toHaveLength(1)
    expect(emitted().details).toHaveLength(1)
  })
})

function toRgb(hex: string): string {
  const value = Number.parseInt(hex.slice(1), 16)
  return `rgb(${(value >> 16) & 255}, ${(value >> 8) & 255}, ${value & 255})`
}
