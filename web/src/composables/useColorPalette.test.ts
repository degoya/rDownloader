/**
 * The colour themes (RD-1240-05): a palette paints Nuxt UI's two scales on `<html>`, the default
 * paints nothing. That every palette passes the contrast rule is `assets/contrast.test.ts`.
 */
import colors from 'tailwindcss/colors'
import { describe, expect, it } from 'vitest'
import { nextTick } from 'vue'

import { applyColorPalette, SHADES, useColorPalette } from './useColorPalette'

function shade(name: string, step: number): string {
  return (colors[name as keyof typeof colors] as Record<number, string>)[step] as string
}

describe('colour themes', () => {
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
    expect(document.documentElement.dataset.palette).toBe('ocean')

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
