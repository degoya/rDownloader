import { useLocalStorage } from '@vueuse/core'
import colors from 'tailwindcss/colors'
import { computed, watch } from 'vue'

/**
 * The colour theme: which accent and which greys the interface is painted in, chosen per
 * browser beside light/dark, which it leaves alone (RD-1240-05, owner 2026-10-10).
 *
 * Nuxt UI paints from `--ui-color-primary-*` and `--ui-color-neutral-*`, written once from the
 * build's `uiTheme.colors`. A palette other than the default sets those eleven shades each on
 * `<html>` itself, which outranks that stylesheet; `main.css` takes the light-mode accent from
 * shade 700 of whatever is set there, so the contrast rule holds for every palette
 * (`contrast.test.ts`). The default sets nothing and is the build's theme exactly.
 */
export const COLOR_PALETTES = {
  signal: null,
  ocean: { primary: 'blue', neutral: 'slate' },
  violet: { primary: 'violet', neutral: 'zinc' },
  forest: { primary: 'emerald', neutral: 'stone' },
  rose: { primary: 'rose', neutral: 'zinc' },
  amber: { primary: 'amber', neutral: 'stone' }
} as const satisfies Record<string, { primary: keyof typeof colors, neutral: keyof typeof colors } | null>

export type ColorPalette = keyof typeof COLOR_PALETTES

export const SHADES = [50, 100, 200, 300, 400, 500, 600, 700, 800, 900, 950] as const

const stored = useLocalStorage<string>('rd-color-palette', 'signal')

/** The stored choice, or the default for anything a browser kept that is no palette (any more). */
function current(): ColorPalette {
  return stored.value in COLOR_PALETTES ? stored.value as ColorPalette : 'signal'
}

function apply(palette: ColorPalette): void {
  const style = document.documentElement.style
  const choice = COLOR_PALETTES[palette]
  for (const key of ['primary', 'neutral'] as const) {
    for (const shade of SHADES) {
      // Concatenated, not a template: `iconBundle.test.ts` reads `i-…-${` as a composed icon name.
      const property = '--ui-color-' + key + '-' + shade
      if (choice) style.setProperty(property, (colors[choice[key]] as Record<number, string>)[shade] ?? '')
      else style.removeProperty(property)
    }
  }
}

/** Paints the stored palette and keeps painting each new choice; called once from `main.ts`. */
export function applyColorPalette(): void {
  watch(current, apply, { immediate: true })
}

/** The accent a palette is recognised by, shade 500 — the default's from the build's own scale. */
export function paletteSwatch(palette: ColorPalette): string {
  const choice = COLOR_PALETTES[palette]
  return choice ? (colors[choice.primary] as Record<number, string>)[500] ?? '' : 'var(--color-signal-500)'
}

export function useColorPalette() {
  const palette = computed<ColorPalette>({
    get: current,
    set: (next) => { stored.value = next }
  })
  return { palette, palettes: Object.keys(COLOR_PALETTES) as ColorPalette[] }
}
