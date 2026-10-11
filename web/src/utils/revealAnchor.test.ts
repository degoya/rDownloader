/**
 * A field the search found stays in view while the page settles (RD-1240-33): opened from another
 * settings page, the bandwidth status above the limits finished loading after the scroll and
 * pushed the focused field below the window, four runs of four.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { revealAnchor } from './revealAnchor'

const settle = (ms: number): Promise<void> => new Promise(resolve => setTimeout(resolve, ms))

let top = 500
const scrolled = vi.fn()

beforeEach(() => {
  top = 500
  scrolled.mockReset()
  Element.prototype.scrollIntoView = scrolled
  document.body.innerHTML = '<section data-settings-anchor="bandwidth.speed_limit"><input /></section>'
  const section = document.querySelector('section') as HTMLElement
  section.getBoundingClientRect = () => ({ top }) as DOMRect
})

afterEach(() => {
  document.body.innerHTML = ''
})

describe('revealAnchor', () => {
  it('scrolls again when the content above moves the found field', async () => {
    expect(await revealAnchor('bandwidth.speed_limit', { focus: true })).toBe(true)
    expect(scrolled).toHaveBeenCalledTimes(1)
    top = 900
    await settle(250)
    expect(scrolled).toHaveBeenCalledTimes(2)
    // Settled: no further jump while nothing moves.
    await settle(250)
    expect(scrolled).toHaveBeenCalledTimes(2)
  })

  it('leaves the reader\'s own scrolling alone', async () => {
    await revealAnchor('bandwidth.speed_limit', { focus: false })
    window.dispatchEvent(new Event('wheel'))
    top = 900
    await settle(250)
    expect(scrolled).toHaveBeenCalledTimes(1)
  })
})
