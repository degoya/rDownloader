import type { NuxtUIOptions } from '@nuxt/ui/vite'

/**
 * The Nuxt UI theme the app is built with, its own module so a test can read it
 * (`uiTheme.test.ts`, RD-1120-09); `vite.config.ts` hands it to the plugin.
 */
export const uiTheme = {
  colors: {
    primary: 'signal',
    secondary: 'cyan',
    neutral: 'slate',
    warning: 'amber',
    error: 'coral'
  },
  // A card stands off the page by its ground, not by an outline on the page's own colour
  // (RD-1101-09); a card nested in another names `variant="outline"` itself. `overflow-clip`
  // instead of Nuxt UI's `overflow-hidden`: a hidden overflow makes the root a scroll
  // container, whose automatic minimum height in a flex column is 0, so in the panel body a
  // card shrank below its content, clipped it and left it out of the scroll height. A clip
  // keeps the rounded corners without that; `min-h-fit` was ignored by Firefox (RD-1110-17).
  card: { slots: { root: 'overflow-clip' }, defaultVariants: { variant: 'soft' } },
  // A number is typed, not stepped (RD-1110-10, `design.md`): only a count one clicks shows its
  // plus and minus, by naming `increment` and `decrement` itself, alike within its group (RD-1140-08).
  inputNumber: { defaultVariants: { increment: false, decrement: false } },
  // An empty state is a dashed outline around Nuxt UI's own padding, one spacing for every
  // one of them (RD-1110-11); a ring cannot be dashed, so the outline is a border.
  empty: { slots: { root: 'border border-dashed border-muted' }, defaultVariants: { variant: 'naked' } },
  // A sub-section divider is the same muted hairline the hand-drawn `border-t` was.
  separator: { variants: { color: { neutral: { border: 'border-muted' } } } },
  // A notice is subtle (174 of 178 named it, RD-1120-14); `soft` and `outline` stay named.
  alert: { defaultVariants: { variant: 'subtle' } },
  // A dialog footer ends right-aligned (`design.md`, *Forms*), named before at all 17 modals.
  modal: { slots: { footer: 'justify-end' } },
  // A toast breaks a long word — a package name without a space — instead of running past its
  // clipped edge (owner, 2026-10-10).
  toast: { slots: { title: 'wrap-anywhere', description: 'wrap-anywhere' } },
  // A tab's content stands off its tab bar; a bar with `:content="false"` renders none.
  // A pill bar wider than its page scrolls sideways with every tab at its full name instead of
  // squeezing them to a letter (RD-1240-33: "E", "Re…", "Status & Li…" at 390 px); where it fits,
  // the tabs still share the line.
  tabs: {
    slots: { content: 'pt-4' },
    compoundVariants: [{
      orientation: 'horizontal',
      variant: 'pill',
      class: { list: 'overflow-x-auto overscroll-x-contain', trigger: 'shrink-0' }
    }]
  },
  // A choice row — one value of a handful, the counts beside it — is a wrapping row of cards
  // without the radio dot, chip-sized (RD-1120-14): `URadioGroup variant="card"
  // indicator="hidden" orientation="horizontal" size="xs"`.
  radioGroup: {
    compoundVariants: [{ variant: 'card', indicator: 'hidden', class: { item: 'px-2 py-1' } }]
  }
} satisfies NonNullable<NuxtUIOptions['ui']>
