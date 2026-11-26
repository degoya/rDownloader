/**
 * The colours the application is really painted with, worked out from the style sheets it ships:
 * `main.css`, Nuxt UI's `index.css`, the sheet Nuxt UI's colour plugin writes into the page, and
 * what `useColorPalette` sets on `<html>`. jsdom knows neither cascade layers nor `color-mix`, so
 * the cascade of custom properties on `<html>` is walked here: inline style, then rules outside
 * any layer, then the layers, each by specificity and order.
 *
 * Until RD-1480-17 the contrast test held copies of the values `main.css` was meant to bind, and
 * so missed that an unlayered `:root` beat Nuxt UI's layered `.dark`.
 */
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'

import { vi } from 'vitest'

import { uiTheme } from '@/uiTheme'

/** Linear-light sRGB plus alpha. */
export interface Colour { channels: [number, number, number], alpha: number }

interface Rule { selectors: string[], layered: boolean, order: number, declarations: Map<string, string> }

/** `{ a { b } }` → the pieces between top-level braces; at-rules without a block are dropped. */
function blocks(css: string): { prelude: string, body: string }[] {
  const found: { prelude: string, body: string }[] = []
  let start = 0
  for (let index = 0; index < css.length; index += 1) {
    if (css[index] === ';') start = index + 1
    if (css[index] !== '{') continue
    let depth = 1
    let end = index + 1
    while (depth > 0 && end < css.length) {
      if (css[end] === '{') depth += 1
      if (css[end] === '}') depth -= 1
      end += 1
    }
    found.push({ prelude: css.slice(start, index).trim(), body: css.slice(index + 1, end - 1) })
    index = end - 1
    start = end
  }
  return found
}

/** A list split at the commas outside parentheses. */
function splitList(text: string): string[] {
  const parts: string[] = []
  let depth = 0
  let start = 0
  for (let index = 0; index < text.length; index += 1) {
    if (text[index] === '(') depth += 1
    if (text[index] === ')') depth -= 1
    if (text[index] === ',' && depth === 0) {
      parts.push(text.slice(start, index).trim())
      start = index + 1
    }
  }
  return [...parts, text.slice(start).trim()]
}

function rulesOf(source: string, layered = false): Omit<Rule, 'order'>[] {
  const css = source.replace(/\/\*[\s\S]*?\*\//g, '')
  return blocks(css).flatMap(({ prelude, body }) => {
    if (prelude.startsWith('@layer')) return rulesOf(body, true)
    // `@theme static` is written out as `:root` inside the theme layer.
    if (prelude.startsWith('@theme')) return [{ selectors: [':root'], layered: true, declarations: declarationsOf(body) }]
    // Media queries and keyframes set no colour token.
    if (prelude.startsWith('@')) return []
    return [{ selectors: splitList(prelude), layered, declarations: declarationsOf(body) }]
  })
}

function declarationsOf(body: string): Map<string, string> {
  return new Map(body.split(';').map(line => line.trim()).filter(Boolean).map((line) => {
    const colon = line.indexOf(':')
    return [line.slice(0, colon).trim(), line.slice(colon + 1).trim()]
  }))
}

function specificity(selector: string): number {
  // Classes, attributes and pseudo-classes; `:not()` itself counts as its argument only.
  return (selector.replace(/:not/g, '').match(/[.:[]/g) ?? []).length
}

/** The sheet Nuxt UI's colour plugin hands to the page's head, from the plugin itself. */
async function nuxtUiColours(): Promise<string> {
  let sheet = ''
  vi.doMock('#imports', () => ({
    defineNuxtPlugin: (setup: unknown) => setup,
    injectHead: () => ({}),
    // Nuxt UI's own defaults under the app's choice (`defaultOptions` of its Vite plugin).
    useAppConfig: () => ({ ui: { colors: { primary: 'green', secondary: 'blue', success: 'green', info: 'blue', warning: 'yellow', error: 'red', neutral: 'slate', ...uiTheme.colors } } }),
    useNuxtApp: () => ({}),
    useHead: (head: { style: { innerHTML: { value: string } }[] }) => { sheet = head.style[0]!.innerHTML.value }
  }))
  const { default: setup } = await import('@nuxt/ui/runtime/plugins/colors.js') as unknown as { default: () => Promise<void> }
  await setup()
  return sheet
}

export const MAIN_CSS = readFileSync('src/assets/main.css', 'utf8')

/** Every rule of the three sheets, in the order the page loads them. */
export async function loadRules(): Promise<Rule[]> {
  const nuxtUi = createRequire(import.meta.url).resolve('@nuxt/ui/runtime/index.css')
  return [await nuxtUiColours(), readFileSync(nuxtUi, 'utf8'), MAIN_CSS]
    .flatMap(sheet => rulesOf(sheet))
    .map((rule, order) => ({ ...rule, order }))
}

/** The value a custom property has on `<html>` as the page stands, before any `var()` is followed. */
function cascaded(rules: Rule[], property: string): string | undefined {
  const root = document.documentElement
  const inline = root.style.getPropertyValue(property)
  if (inline) return inline
  const candidates = rules.flatMap((rule) => {
    const value = rule.declarations.get(property)
    if (value === undefined) return []
    return rule.selectors
      .filter(selector => selector !== ':host' && root.matches(selector))
      .map(selector => ({ value, weight: [rule.layered ? 0 : 1, specificity(selector), rule.order] as const }))
  })
  candidates.sort((left, right) => left.weight[0] - right.weight[0] || left.weight[1] - right.weight[1] || left.weight[2] - right.weight[2])
  return candidates.at(-1)?.value
}

const toLinear = (value: number): number => value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4
const toGamma = (value: number): number => value <= 0.0031308 ? value * 12.92 : 1.055 * value ** (1 / 2.4) - 0.055

/** Follows `var()` to its colour; throws on anything it does not know, so nothing passes unseen. */
export function resolveColour(rules: Rule[], value: string): Colour {
  const text = value.trim()
  const reference = /^var\(\s*(--[\w-]+)\s*(?:,([\s\S]*))?\)$/.exec(text)
  if (reference) {
    const next = cascaded(rules, reference[1]!) ?? reference[2]
    if (next === undefined) throw new Error(`${reference[1]} is set nowhere`)
    return resolveColour(rules, next)
  }
  const hex = /^#([\da-f]{3}|[\da-f]{6})$/i.exec(text)?.[1]
  if (hex) {
    const full = hex.length === 3 ? [...hex].map(digit => digit + digit).join('') : hex
    return { alpha: 1, channels: [0, 2, 4].map(at => toLinear(Number.parseInt(full.slice(at, at + 2), 16) / 255)) as Colour['channels'] }
  }
  const oklch = /^oklch\(([\d.]+)% ([\d.]+) ([\d.]+|none)\)$/.exec(text)
  if (!oklch) throw new Error(`not a colour this test can read: ${text}`)
  const [lightness, chroma, hue] = [Number(oklch[1]) / 100, Number(oklch[2]), (Number(oklch[3]) || 0) * Math.PI / 180]
  const a = chroma * Math.cos(hue)
  const b = chroma * Math.sin(hue)
  const l = (lightness + 0.3963377774 * a + 0.2158037573 * b) ** 3
  const m = (lightness - 0.1055613458 * a - 0.0638541728 * b) ** 3
  const s = (lightness - 0.0894841775 * a - 1.291485548 * b) ** 3
  return {
    alpha: 1,
    channels: [
      4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
      -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
      -0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s
    ].map(channel => Math.min(1, Math.max(0, channel))) as Colour['channels']
  }
}

/** `color` at `alpha` over an opaque ground, as a browser composites it. */
export function over(color: Colour, alpha: number, ground: Colour): Colour {
  const channels = color.channels.map((channel, at) =>
    toLinear(toGamma(channel) * alpha + toGamma(ground.channels[at]!) * (1 - alpha))) as Colour['channels']
  return { channels, alpha: 1 }
}

/** The WCAG 2.x contrast ratio of two opaque colours. */
export function contrast(left: Colour, right: Colour): number {
  const [low, high] = [left, right]
    .map(({ channels: [red, green, blue] }) => 0.2126 * red + 0.7152 * green + 0.0722 * blue)
    .sort((x, y) => x - y) as [number, number]
  return (high + 0.05) / (low + 0.05)
}
