/**
 * The colour themes (RD-1240-05): a palette paints Nuxt UI's two scales on `<html>`, the default
 * paints nothing, and every palette's accents pass the contrast rule `main.css` holds the default
 * to (`assets/contrast.test.ts`): shade 700 in light mode, as text and under white, shade 400 in
 * dark mode on the palette's own dark ground.
 */
import colors from 'tailwindcss/colors'
import { describe, expect, it } from 'vitest'
import { nextTick } from 'vue'

import { applyColorPalette, COLOR_PALETTES, SHADES, useColorPalette } from './useColorPalette'

/** Relative luminance (WCAG 2.x) of a Tailwind 4 `oklch(L% C H)` value, through linear sRGB. */
function luminance(value: string): number {
  const match = /oklch\(([\d.]+)% ([\d.]+) ([\d.]+|none)\)/.exec(value)
  if (!match) throw new Error(`not an oklch colour: ${value}`)
  const [lightness, chroma, hue] = [Number(match[1]) / 100, Number(match[2]), (Number(match[3]) || 0) * Math.PI / 180]
  const a = chroma * Math.cos(hue)
  const b = chroma * Math.sin(hue)
  const l = (lightness + 0.3963377774 * a + 0.2158037573 * b) ** 3
  const m = (lightness - 0.1055613458 * a - 0.0638541728 * b) ** 3
  const s = (lightness - 0.0894841775 * a - 1.291485548 * b) ** 3
  const clamp = (channel: number): number => Math.min(1, Math.max(0, channel))
  const red = clamp(4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s)
  const green = clamp(-1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s)
  const blue = clamp(-0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s)
  return 0.2126 * red + 0.7152 * green + 0.0722 * blue
}

function contrast(left: string, right: string): number {
  const [high, low] = [luminance(left), luminance(right)].sort((x, y) => y - x) as [number, number]
  return (high + 0.05) / (low + 0.05)
}

const WHITE = 'oklch(100% 0 0)'
const AA_TEXT = 4.5

function shade(name: string, step: number): string {
  return (colors[name as keyof typeof colors] as Record<number, string>)[step] as string
}

describe('colour themes', () => {
  it.each(Object.entries(COLOR_PALETTES).filter(([, choice]) => choice))('%s keeps the accents readable', (_, choice) => {
    const { primary, neutral } = choice as { primary: string, neutral: string }
    expect(contrast(shade(primary, 700), WHITE)).toBeGreaterThanOrEqual(AA_TEXT)
    expect(contrast(shade(primary, 700), shade(neutral, 50))).toBeGreaterThanOrEqual(AA_TEXT)
    expect(contrast(shade(primary, 400), shade(neutral, 900))).toBeGreaterThanOrEqual(AA_TEXT)
  })

  it('paints both scales of a chosen palette on <html> and takes them away for the default', async () => {
    const style = document.documentElement.style
    applyColorPalette()
    const { palette } = useColorPalette()
    expect(palette.value).toBe('signal')
    expect(style.getPropertyValue('--ui-color-primary-500')).toBe('')

    palette.value = 'ocean'
    await nextTick()
    for (const step of SHADES) {
      expect(style.getPropertyValue(`--ui-color-primary-${step}`)).toBe(shade('blue', step))
      expect(style.getPropertyValue(`--ui-color-neutral-${step}`)).toBe(shade('slate', step))
    }
    expect(localStorage.getItem('rd-color-palette')).toBe('ocean')

    palette.value = 'signal'
    await nextTick()
    expect(style.getPropertyValue('--ui-color-primary-700')).toBe('')
    expect(style.getPropertyValue('--ui-color-neutral-700')).toBe('')
  })

  it('reads a stored name that is no palette as the default', async () => {
    localStorage.setItem('rd-color-palette', 'neon')
    window.dispatchEvent(new StorageEvent('storage', { key: 'rd-color-palette', newValue: 'neon', storageArea: localStorage }))
    await nextTick()
    expect(useColorPalette().palette.value).toBe('signal')
  })
})
