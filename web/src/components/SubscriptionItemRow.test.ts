import { fireEvent, screen } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'
import { defineComponent, watch } from 'vue'

import type { SubscriptionItem } from '@/api/types'
import subscriptions from '@/locales/en/subscriptions.json'
import { mountComponent, passthrough } from '@/test/mount'

import SubscriptionItemRow from './SubscriptionItemRow.vue'

function item(attributes: Record<string, string> = {}): SubscriptionItem {
  return { id: 'i1', title: 'Some.Movie.2024.1080p', state: 'pending', attributes } as SubscriptionItem
}

function mount(props: { item: SubscriptionItem, showImages?: boolean }) {
  return mountComponent(SubscriptionItemRow, {
    props,
    messages: { subscriptions },
    stubs: {
      // Keeps the controlled overlay observable without pulling Nuxt's portal into jsdom -
      // and reports its state back the way the real one does. The silent version of this stub
      // hid a defect for a release: Nuxt UI emits `update:open(true)` whenever the controlled
      // prop opens it, a hover included, and the row pinned the cover on that.
      UPopover: {
        props: ['open'],
        emits: ['update:open'],
        setup(props: { open: boolean }, { emit }: { emit: (event: 'update:open', value: boolean) => void }) {
          watch(() => props.open, (value) => emit('update:open', value))
        },
        template: '<div><slot /><div v-if="open"><slot name="content" /></div></div>'
      },
      ULink: passthrough
    }
  })
}

describe('SubscriptionItemRow', () => {
  it('shows the size in the row, so most choices need no expanding', () => {
    mount({ item: item({ size: '4509715660' }) })
    expect(screen.getByText(/4\.2|4,2/)).toBeTruthy()
  })

  it('offers no chevron when the indexer said nothing about the hit', () => {
    mount({ item: item() })
    expect(screen.queryByRole('button', { name: 'Details' })).toBeNull()
  })

  it('shows IMDb and language before expanding, with the remaining metadata behind the chevron', async () => {
    mount({ item: item({ imdbscore: '7.8', language: 'de', resolution: '1080p' }) })
    expect(screen.getByText('7.8')).toBeTruthy()
    expect(screen.getByText('de')).toBeTruthy()
    expect(screen.queryByText('1080p')).toBeNull()

    await fireEvent.click(screen.getByRole('button', { name: 'Details' }))
    expect(screen.getByText('1080p')).toBeTruthy()
    expect(screen.getAllByText('7.8')).toHaveLength(1)
  })

  it('shows the genre before expanding, for music as for anything else, and only once', async () => {
    mount({ item: item({ genre: 'Progressive Rock', resolution: 'FLAC' }) })
    expect(screen.getByText('Progressive Rock')).toBeTruthy()

    await fireEvent.click(screen.getByRole('button', { name: 'Details' }))
    expect(screen.getByText('FLAC')).toBeTruthy()
    expect(screen.getAllByText('Progressive Rock')).toHaveLength(1)
  })

  it('offers no chevron for a genre alone, since the row already shows it', () => {
    mount({ item: item({ genre: 'Drama' }) })
    expect(screen.getByText('Drama')).toBeTruthy()
    expect(screen.queryByRole('button', { name: 'Details' })).toBeNull()
  })

  it('renders the cover, and drops it when images are switched off', () => {
    const attributes = { coverurl: 'https://indexer.test/c.jpg' }
    const { container, unmount } = mount({ item: item(attributes), showImages: true })
    expect(container.querySelector('img')).not.toBeNull()
    expect(screen.queryByTestId('cover-placeholder')).toBeNull()
    unmount()

    const off = mount({ item: item(attributes), showImages: false })
    expect(off.container.querySelector('img')).toBeNull()
  })

  it('stands a placeholder where there is no cover, so the titles stay in one column', () => {
    const { container } = mount({ item: item(), showImages: true })
    expect(container.querySelector('img')).toBeNull()
    const placeholder = screen.getByTestId('cover-placeholder')
    expect(placeholder.getAttribute('aria-hidden')).toBe('true')
    expect(placeholder.className).toContain('size-12')
  })

  it('marks a protected archive from the flag without inventing a password', () => {
    // Newznab's `password` attribute is only 0/1/2, not the secret.
    mount({ item: item({ password: '1' }) })
    expect(screen.getByTitle('The archive is password protected')).toBeTruthy()
    expect(screen.queryByText('password')).toBeNull()
  })

  it('does not mark an unprotected archive', () => {
    mount({ item: item({ password: '0' }) })
    expect(screen.queryByTitle('The archive is password protected')).toBeNull()
  })
})

/**
 * RD-110-27: the title is the row's first obligation.
 *
 * The row wrapped, but its title was `flex-1 min-w-0` — the one item allowed to shrink to
 * nothing — so on a narrow window everything else kept its width and the title went first.
 * jsdom lays nothing out, so what can be asserted is the rule the browser applies: the row
 * wraps, and the title claims the 200 px the accounting in `design.md` reserves for a name,
 * which is what pushes the rest under it instead of over it.
 */
describe('SubscriptionItemRow, width', () => {
  it('wraps what does not fit under the title instead of squeezing the title', () => {
    mount({ item: item({ size: '4509715660' }) })
    const title = screen.getByText('Some.Movie.2024.1080p')
    const row = title.closest('li')?.firstElementChild as HTMLElement
    expect(row.classList.contains('flex-wrap')).toBe(true)
    const cell = title.parentElement as HTMLElement
    expect(cell.classList.contains('basis-[200px]')).toBe(true)
    expect(cell.classList.contains('min-w-0')).toBe(true)
  })

  it('says under the row why a hit was skipped, off the title\u2019s line', () => {
    mount({ item: { ...item(), state: 'skipped', reason: 'title_excluded' } as SubscriptionItem })
    const reason = screen.getByText(subscriptions.reasons.title_excluded)
    // Below the row, not in it: a sentence in the row is taken from the title.
    const row = screen.getByText('Some.Movie.2024.1080p').closest('li')?.firstElementChild
    expect(row?.contains(reason)).toBe(false)
  })

  it('says nothing under a hit that was not skipped', () => {
    mount({ item: item() })
    expect(screen.queryByText(subscriptions.reasons.title_excluded)).toBeNull()
  })
})

describe('SubscriptionItemRow, password marker', () => {
  it('removes the marker from the title and shows the stored password separately', () => {
    mount({
      item: {
        id: 'i2',
        title: 'Some.Series.S02E05{{unlockme}}',
        password: 'unlockme',
        state: 'pending'
      } as SubscriptionItem
    })
    expect(screen.getByText('Some.Series.S02E05')).toBeTruthy()
    expect(screen.getByText('unlockme')).toBeTruthy()
  })
})

/**
 * RD-106-08: the cover can be looked at, in the row, and not only with a mouse.
 *
 * At `size-12` a cover decides nothing, and the large copy sat in the expanded detail — the
 * useful one behind a chevron, the useless one in sight. Hover is the pointer's way in;
 * focus and a tap have to reach the same place, or the feature does not exist for a part of
 * the audience (`docs/accessibility.md`, WCAG 2.1.1).
 */
describe('SubscriptionItemRow, the cover enlarged', () => {
  const withCover = { coverurl: 'https://indexer.test/c.jpg' }

  const trigger = () => screen.getByRole('button', { name: 'Show the cover larger' })

  it('has no enlarging control for a hit without a cover', () => {
    mount({ item: item({ size: '100' }) })
    expect(screen.queryByRole('button', { name: 'Show the cover larger' })).toBeNull()
  })

  it('shows the cover large while the pointer is on it, and hides it again after', async () => {
    mount({ item: item(withCover), showImages: true })
    expect(screen.queryByTestId('cover-preview')).toBeNull()

    await fireEvent.mouseEnter(trigger())
    expect(screen.getByTestId('cover-preview')).toBeTruthy()

    await fireEvent.mouseLeave(screen.getByRole('button', { name: 'Close the enlarged cover' }))
    expect(screen.queryByTestId('cover-preview')).toBeNull()
  })

  it('bounds the enlarged cover by the space the popover actually has, not a fixed guess', async () => {
    // The popover already flips side and align to keep itself inside the window (a row at the
    // very top or the very bottom lands differently), and exposes how much room that placement
    // leaves as a CSS custom property. Binding the height to it - instead of a static `vh` - is
    // what lets the image use that space at both ends of the list. `0.75rem` gives back what the
    // content wrapper's own `p-1` padding and border take out of that measured space.
    mount({ item: item(withCover), showImages: true })
    await fireEvent.mouseEnter(trigger())

    const preview = screen.getByTestId('cover-preview')
    // `h-`, not `max-h-`: a cap alone never enlarges anything, and these covers arrive smaller
    // than the window, so the first attempt at this changed one ceiling for a better ceiling
    // and the picture stayed exactly as small as before.
    expect(preview.className).toContain(
      'h-[calc(var(--reka-popover-content-available-height)-0.75rem)]'
    )
    expect(preview.className).not.toContain('max-h-[')
    expect(preview.className).not.toContain('vh]')
    // Width follows the aspect ratio from that height and is capped only against the window,
    // so a portrait cover is not letterboxed by a cap narrower than it needs.
    expect(preview.className).toContain('max-w-[min(48rem,calc(100vw-2rem))]')
  })

  it('shows it on keyboard focus as well, and Escape closes it', async () => {
    mount({ item: item(withCover), showImages: true })

    await fireEvent.focus(trigger())
    expect(screen.getByTestId('cover-preview')).toBeTruthy()

    const button = screen.getByRole('button', { name: 'Close the enlarged cover' })
    await fireEvent.keyDown(button, { key: 'Escape' })
    expect(screen.queryByTestId('cover-preview')).toBeNull()
    // Nothing was focused but the button itself, so there is no focus to hand back.
    expect(button.isConnected).toBe(true)
  })

  it('opens on a tap and closes on the next one', async () => {
    // A touch device has no hover at all, so the click has to carry the whole interaction.
    mount({ item: item(withCover), showImages: true })

    await fireEvent.click(trigger())
    expect(screen.getByTestId('cover-preview')).toBeTruthy()

    await fireEvent.click(screen.getByRole('button', { name: 'Close the enlarged cover' }))
    expect(screen.queryByTestId('cover-preview')).toBeNull()
  })

  it('says what the control does and whether it is open', async () => {
    mount({ item: item(withCover), showImages: true })
    expect(trigger().getAttribute('aria-expanded')).toBe('false')

    await fireEvent.click(trigger())
    expect(
      screen.getByRole('button', { name: 'Close the enlarged cover' }).getAttribute('aria-expanded')
    ).toBe('true')
  })

  it('offers no enlargement when external images are switched off', async () => {
    // The setting exists because an indexer's address tells that server what is on screen;
    // a picture that is not fetched small must not be fetched large either.
    mount({ item: item(withCover), showImages: false })
    expect(screen.queryByRole('button', { name: 'Show the cover larger' })).toBeNull()
  })

  it('leaves a gap when the address does not load, rather than a broken control', async () => {
    const { container } = mount({ item: item(withCover), showImages: true })

    const image = container.querySelector('img')
    expect(image).not.toBeNull()
    await fireEvent.error(image as HTMLImageElement)

    expect(container.querySelector('img')).toBeNull()
    expect(screen.queryByRole('button', { name: 'Show the cover larger' })).toBeNull()
  })

  it('does not show the cover a second time in the expanded detail', async () => {
    const { container } = mount({
      item: item({ ...withCover, resolution: '1080p' }),
      showImages: true
    })

    await fireEvent.click(screen.getByRole('button', { name: 'Details' }))

    const sources = [...container.querySelectorAll('img')].map(image => image.getAttribute('src'))
    expect(sources).toEqual([withCover.coverurl])
  })
})

/**
 * RD-107-16: one cover at a time, across every row on screen.
 *
 * Each row used to hold its own `hovered`/`pinned`, so two covers stood open at once — a
 * pinned one in one row beside a hovered one in another, and a fast traversal where the
 * leaving row's `mouseleave` arrived after the next row's `mouseenter`. The rule is in
 * `design.md`: a pointer or focus takes the single slot for as long as it lasts, a pin keeps
 * it for the rest of the time, and only another deliberate act replaces a pin.
 */
describe('SubscriptionItemRow, only one cover open', () => {
  const cover = 'https://indexer.test/c.jpg'

  function hit(id: string, title: string): SubscriptionItem {
    // Same shape as `item()` above: the attributes go in as a plain string map, which is what
    // the cast needs to stay within reach of the real type.
    const attributes: Record<string, string> = { coverurl: cover }
    return { id, title, state: 'pending', attributes } as SubscriptionItem
  }

  /** Two rows in one list, the way the review list and the archive draw them. */
  function mountPair() {
    const Pair = defineComponent({
      components: { SubscriptionItemRow },
      data: () => ({ first: hit('i1', 'First.Movie.2024'), second: hit('i2', 'Second.Movie.2023') }),
      template: '<ul>'
        + '<SubscriptionItemRow :item="first" :show-images="true" />'
        + '<SubscriptionItemRow :item="second" :show-images="true" />'
        + '</ul>'
    })
    return mountComponent(Pair, {
      messages: { subscriptions },
      stubs: {
        UPopover: {
          props: ['open'],
          emits: ['update:open'],
          setup(props: { open: boolean }, { emit }: { emit: (event: 'update:open', value: boolean) => void }) {
            watch(() => props.open, (value) => emit('update:open', value))
          },
          template: '<div><slot /><div v-if="open"><slot name="content" /></div></div>'
        },
        ULink: passthrough
      }
    })
  }

  /** Neither row is expandable, so the only buttons in the list are the two cover triggers. */
  function trigger(index: number): HTMLElement {
    const found = screen.getAllByRole('button')[index]
    if (!found) throw new Error(`no cover trigger at ${index}`)
    return found
  }
  /** The title of whatever single cover stands open; `null` when none does. */
  function openCover(): string | null {
    const previews = screen.queryAllByTestId('cover-preview')
    expect(previews.length).toBeLessThanOrEqual(1)
    return previews[0]?.getAttribute('alt') ?? null
  }

  it('lets the next row take the cover, even when the first row leaves late', async () => {
    mountPair()
    const first = trigger(0)
    const second = trigger(1)

    await fireEvent.mouseEnter(first)
    expect(openCover()).toBe('First.Movie.2024')

    // The next row is entered before the previous one is left — the reported overlap.
    await fireEvent.mouseEnter(second)
    expect(screen.getAllByTestId('cover-preview')).toHaveLength(1)
    expect(openCover()).toBe('Second.Movie.2023')

    // The late `mouseleave` belongs to a row that no longer holds the slot and changes nothing.
    await fireEvent.mouseLeave(first)
    expect(openCover()).toBe('Second.Movie.2023')
  })

  it('keeps a pinned cover through a pass-by and shows it again when the pointer moves on', async () => {
    mountPair()
    const first = trigger(0)
    const second = trigger(1)

    await fireEvent.click(first)
    expect(openCover()).toBe('First.Movie.2024')

    // A pointer crossing the list is not a decision: it borrows the slot, it does not take it.
    await fireEvent.mouseEnter(second)
    expect(openCover()).toBe('Second.Movie.2023')

    await fireEvent.mouseLeave(second)
    expect(openCover()).toBe('First.Movie.2024')
  })

  it('replaces the pin when another row is tapped', async () => {
    mountPair()
    const first = trigger(0)
    const second = trigger(1)

    await fireEvent.click(first)
    await fireEvent.click(second)
    expect(openCover()).toBe('Second.Movie.2023')

    // The first pin is gone rather than waiting underneath.
    await fireEvent.click(second)
    expect(openCover()).toBeNull()
  })

  it('carries the single cover along with keyboard focus, and Escape closes it', async () => {
    // The keyboard path has to reach exactly what the pointer reaches (WCAG 2.1.1).
    mountPair()
    const first = trigger(0)
    const second = trigger(1)

    await fireEvent.focus(first)
    expect(openCover()).toBe('First.Movie.2024')

    await fireEvent.blur(first)
    await fireEvent.focus(second)
    expect(screen.getAllByTestId('cover-preview')).toHaveLength(1)
    expect(openCover()).toBe('Second.Movie.2023')

    await fireEvent.keyDown(second, { key: 'Escape' })
    expect(openCover()).toBeNull()
  })

  it('does not let a row whose address failed hold the slot against its neighbour', async () => {
    const { container } = mountPair()
    const first = trigger(0)
    const second = trigger(1)

    await fireEvent.mouseEnter(first)
    const thumbnail = container.querySelector('img') as HTMLImageElement
    await fireEvent.error(thumbnail)

    await fireEvent.mouseEnter(second)
    expect(openCover()).toBe('Second.Movie.2023')
  })
})
