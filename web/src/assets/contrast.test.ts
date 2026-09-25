/**
 * Contrast of the accent scales (WCAG 2.2, 1.4.3).
 *
 * axe-core cannot answer this in a test run: its colour-contrast rule needs a canvas to sample
 * rendered pixels, and jsdom has none. So the ratios are computed from the palette instead,
 * which is the same arithmetic and does not depend on anything being rendered.
 *
 * The ratios are checked against the AA threshold rather than pinned to exact values: a palette
 * edit that quietly drops an accent below 4.5:1 is exactly the change this is here to catch.
 */
import { describe, expect, it } from 'vitest'

/** Relative luminance, per WCAG 2.x. */
function luminance(hex: string): number {
  const value = hex.replace('#', '')
  const channels = [0, 2, 4].map(offset => Number.parseInt(value.slice(offset, offset + 2), 16) / 255)
  const linear = channels.map(channel =>
    channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4
  )
  return 0.2126 * linear[0]! + 0.7152 * linear[1]! + 0.0722 * linear[2]!
}

function contrast(left: string, right: string): number {
  const [high, low] = [luminance(left), luminance(right)].sort((a, b) => b - a) as [number, number]
  return (high + 0.05) / (low + 0.05)
}

/** The two grounds the application actually paints, from `useTheme`. */
const LIGHT = '#f8fafc'
const DARK = '#08151d'

/** The accent shades `main.css` binds to `--ui-primary` and `--ui-error`. */
const LIGHT_PRIMARY = '#117572'
const LIGHT_ERROR = '#ae2c27'
const DARK_PRIMARY = '#20ded4'
const DARK_ERROR = '#fa746d'

/** WCAG AA for text below 18.66px bold or 24px regular. */
const AA_TEXT = 4.5

describe('accent contrast', () => {
  it('the light-mode accents are readable as text and as a ground under white', () => {
    expect(contrast(LIGHT_PRIMARY, LIGHT)).toBeGreaterThanOrEqual(AA_TEXT)
    expect(contrast(LIGHT_ERROR, LIGHT)).toBeGreaterThanOrEqual(AA_TEXT)
    // Also as a button ground: the label on a primary button is white.
    expect(contrast(LIGHT_PRIMARY, '#ffffff')).toBeGreaterThanOrEqual(AA_TEXT)
    expect(contrast(LIGHT_ERROR, '#ffffff')).toBeGreaterThanOrEqual(AA_TEXT)
  })

  it('the dark-mode accents are readable as text', () => {
    expect(contrast(DARK_PRIMARY, DARK)).toBeGreaterThanOrEqual(AA_TEXT)
    expect(contrast(DARK_ERROR, DARK)).toBeGreaterThanOrEqual(AA_TEXT)
  })

  it('the shades Nuxt UI would have chosen are the ones that failed', () => {
    // Recorded so the override in `main.css` reads as a fix rather than a preference: shade
    // 500 is what the library picks for light mode, and it does not clear the threshold.
    expect(contrast('#14b8b0', LIGHT)).toBeLessThan(AA_TEXT)
    expect(contrast('#e9524b', LIGHT)).toBeLessThan(AA_TEXT)
  })
})
