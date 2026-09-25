import { screen } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'

import subscriptions from '@/locales/en/subscriptions.json'
import { mountComponent, passthrough } from '@/test/mount'

import SubscriptionItemDetails from './SubscriptionItemDetails.vue'

function mount(attributes: Record<string, string>, showImages = true) {
  return mountComponent(SubscriptionItemDetails, {
    props: { attributes, showImages },
    messages: { subscriptions },
    // Not in the shared stubs: this is the only component here that renders a link.
    stubs: { ULink: passthrough }
  })
}

describe('SubscriptionItemDetails', () => {
  it('keeps attributes it has no field for, rather than hiding them', () => {
    // The point of not using an allow-list: indexers disagree about what they emit, and the
    // one field a given indexer considers most useful may be one nobody anticipated.
    mount({ nonstandard_thing: 'worth seeing', grabs: '12' })
    expect(screen.getByText('nonstandard_thing')).toBeTruthy()
    expect(screen.getByText('worth seeing')).toBeTruthy()
    // A named one is laid out instead, not repeated below.
    expect(screen.queryByText('grabs')).toBeNull()
    expect(screen.getByText('12')).toBeTruthy()
  })

  it('joins season and episode into one readable figure', () => {
    mount({ season: '2', episode: '5' })
    expect(screen.getByText('S02E05')).toBeTruthy()
  })

  it('links to IMDb only when the id is the bare number newznab sends', () => {
    const { container, unmount } = mount({ imdb: '0111161' })
    expect(container.textContent).toContain('Open on IMDb')
    unmount()

    // Anything else is not turned into an address.
    expect(mount({ imdb: 'tt0111161' }).container.textContent).not.toContain('Open on IMDb')
  })

  it('says so plainly when there is nothing to show', () => {
    mount({})
    expect(screen.getByText('The indexer said nothing more about this hit.')).toBeTruthy()
  })

  it('leaves the backdrop out when images are switched off', () => {
    const on = mount({ backdropcoverurl: 'https://indexer.test/b.jpg' }, true)
    expect(on.container.querySelector('img')).not.toBeNull()
    on.unmount()

    const off = mount({ backdropcoverurl: 'https://indexer.test/b.jpg' }, false)
    expect(off.container.querySelector('img')).toBeNull()
  })
})

/**
 * RD-106-08: the cover is shown in the row now, and only there.
 *
 * It used to stand twice — a thumbnail in the row and a larger copy here — so the useful
 * size sat behind the chevron. The backdrop is a different picture and is not affected.
 */
describe('SubscriptionItemDetails, the cover', () => {
  it('does not render the cover a second time', () => {
    const { container } = mount({ coverurl: 'https://indexer.test/c.jpg', grabs: '12' })
    expect(container.querySelector('img')).toBeNull()
  })

  it('does not fall back to listing the cover address as a raw attribute', () => {
    // It stays in the named list, or removing the picture would print the URL instead.
    mount({ coverurl: 'https://indexer.test/c.jpg' })
    expect(screen.queryByText('coverurl')).toBeNull()
    expect(screen.queryByText('https://indexer.test/c.jpg')).toBeNull()
  })

  it('still shows the backdrop banner, which is a different picture', () => {
    const { container } = mount({
      coverurl: 'https://indexer.test/c.jpg',
      backdropcoverurl: 'https://indexer.test/b.jpg'
    })
    const sources = [...container.querySelectorAll('img')].map(image => image.getAttribute('src'))
    expect(sources).toEqual(['https://indexer.test/b.jpg'])
  })
})
