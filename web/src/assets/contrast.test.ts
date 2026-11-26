/**
 * Contrast of the rendered colours (WCAG 2.2, 1.4.3 and 1.4.11), in light and dark mode and in
 * every colour theme.
 *
 * axe-core cannot answer this in a test run: its colour-contrast rule needs a canvas to sample
 * rendered pixels, and jsdom has none. So the colours are read out of the style sheets the page
 * loads, through the cascade (`test/renderedColors.ts`), never out of constants copied here: a
 * copy stays right while the sheet goes wrong (A11Y-16, RD-1480-17).
 *
 * The ratios are checked against the AA threshold rather than pinned to exact values: an edit
 * that quietly drops a colour below 4.5:1 is exactly the change this is here to catch.
 */
import { afterAll, beforeAll, beforeEach, describe, expect, it } from 'vitest'
import { nextTick } from 'vue'

import { applyColorPalette, COLOR_PALETTES, useColorPalette } from '@/composables/useColorPalette'
import { contrast, loadRules, MAIN_CSS, over, resolveColour, type Colour } from '@/test/renderedColors'
import { uiTheme } from '@/uiTheme'

/** WCAG AA for text below 18.66px bold or 24px regular. */
const AA_TEXT = 4.5
/** WCAG AA for a component's own graphics: the focus ring. */
const AA_GRAPHIC = 3

const ACCENTS = ['primary', 'success', 'info', 'warning', 'error'] as const
/** The grounds text stands on: the page, a card, a row, a badge. */
const GROUNDS = ['bg', 'bg-muted', 'bg-elevated'] as const
const PALETTES = Object.keys(COLOR_PALETTES) as (keyof typeof COLOR_PALETTES)[]

let rules: Awaited<ReturnType<typeof loadRules>>
const token = (name: string): Colour => resolveColour(rules, `var(--ui-${name})`)

beforeAll(async () => {
  rules = await loadRules()
  applyColorPalette()
})

afterAll(() => {
  document.documentElement.className = ''
})

describe.each(['light', 'dark'])('%s mode', (mode) => {
  describe.each(PALETTES)('%s', (palette) => {
    beforeEach(async () => {
      document.documentElement.className = mode
      useColorPalette().palette.value = palette
      await nextTick()
    })

    it('every accent is readable as text on every ground and on its own tint', () => {
      const short: string[] = []
      for (const accent of ACCENTS) {
        for (const ground of GROUNDS) {
          const plain = contrast(token(accent), token(ground))
          if (plain < AA_TEXT) short.push(`${accent} on ${ground}: ${plain.toFixed(2)}`)
          // `soft` and `subtle` badges, buttons and notices: `bg-<accent>/10 text-<accent>`.
          const tint = over(token(accent), 0.1, token(ground))
          const tinted = contrast(token(accent), tint)
          if (tinted < AA_TEXT) short.push(`${accent} on its tint over ${ground}: ${tinted.toFixed(2)}`)
        }
      }
      expect(short).toEqual([])
    })

    it('the label of a solid button is readable on every accent', () => {
      for (const accent of ACCENTS) {
        expect(contrast(token('text-inverted'), token(accent)), accent).toBeGreaterThanOrEqual(AA_TEXT)
      }
    })

    it('the greys of running text are readable on every ground', () => {
      for (const text of ['text', 'text-muted', 'text-toned', 'text-highlighted']) {
        for (const ground of GROUNDS) {
          expect(contrast(token(text), token(ground)), `${text} on ${ground}`).toBeGreaterThanOrEqual(AA_TEXT)
        }
      }
    })

    it('the focus ring stands out from every ground', () => {
      for (const ground of GROUNDS) {
        expect(contrast(token('primary'), token(ground)), ground).toBeGreaterThanOrEqual(AA_GRAPHIC)
      }
    })
  })
})

it('a focused control is outlined in the full accent, not a quarter of it', () => {
  // Nuxt UI draws `outline-primary/25` (1.43:1 in light mode); the navigation draws it on `::before`.
  expect(MAIN_CSS).toMatch(/:focus-visible,\s*:focus-visible::before \{\s*outline-color: var\(--ui-primary\) !important;/)
})

it('a notice paints its description in the full colour', () => {
  // Nuxt UI's `opacity-90` costs the accent a tenth of its contrast.
  expect(uiTheme.alert.slots.description).toBe('opacity-100')
})
