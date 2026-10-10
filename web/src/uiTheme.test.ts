/**
 * The theme's card fix stays in place (RD-1120-09).
 *
 * A card's root clips instead of hiding its overflow: `overflow-hidden` makes it a scroll
 * container whose minimum height in a flex column is 0, and the card shrank below its content in
 * the panel body (`uiTheme.ts`). Fixed twice before (RD-1101-19, RD-1110-17); jsdom lays nothing
 * out, so this holds the class itself.
 */
import { describe, expect, it } from 'vitest'

import { uiTheme } from './uiTheme'

describe('the Nuxt UI theme', () => {
  it('clips a card instead of hiding its overflow', () => {
    const root = uiTheme.card.slots.root.split(/\s+/)
    expect(root).toContain('overflow-clip')
    expect(root).not.toContain('overflow-hidden')
  })

  // RD-1120-14: the templates stopped naming these, so the theme is the only place left that does.
  it('breaks a long word in a toast instead of clipping it', () => {
    expect(uiTheme.toast.slots.title).toContain('wrap-anywhere')
    expect(uiTheme.toast.slots.description).toContain('wrap-anywhere')
  })

  it('carries the defaults the templates no longer repeat', () => {
    expect(uiTheme.alert.defaultVariants.variant).toBe('subtle')
    expect(uiTheme.modal.slots.footer).toBe('justify-end')
    expect(uiTheme.tabs.slots.content).toBe('pt-4')
  })
})
