# Accessibility

rDownloader's web interface targets **WCAG 2.2 level AA**. This page records what that means
here, what has been verified and how, and — as honestly as the rest — what has not.

## What was found and fixed

Three real defects, not hypotheticals:

- **The accent colour failed contrast in light mode.** Nuxt UI takes shade 500 of the
  configured palette as the accent. Against this application's light ground that is 2.36:1 for
  `signal` and 3.48:1 for `coral` — below the 4.5:1 text needs, and below it again as a button
  ground under white text. `--ui-primary` and `--ui-error` are now bound to shade 700 in light
  mode (5.27:1 and 6.29:1); dark mode keeps the library's shade 400, which is 10.98:1 and
  6.86:1 on the dark ground.
- **A progress bar had no accessible name.** The post-processing step list rendered a bare
  `role="progressbar"`, which a screen reader announces as a percentage attached to nothing.
  It now names the step it belongs to.
- **The connection indicator was a pulse and nothing else.** The animation is now
  `aria-hidden` with the meaning beside it as text, so it says "connected" rather than being a
  dot that moves.

## What is in place

| Requirement | How |
| --- | --- |
| Skip link (2.4.1) | First element in the tab order on every page; jumps to `#main-content`, which is focusable. |
| Landmarks (1.3.1) | `<nav aria-label>` around the sidebar menu, `<main id="main-content">` around the routed view. |
| Live updates (4.1.3) | One polite `role="status"` region announces the queue as a summary — "3 of 9 packages downloading" — not one message per download, which would be unbearable during a busy queue. |
| Reduced motion (2.3.3) | `prefers-reduced-motion: reduce` collapses every animation and transition. Everything animated here is feedback that also exists as text or as a static state. |
| Contrast (1.4.3) | Ratios computed from the palette and checked against the 4.5:1 AA threshold in `web/src/assets/contrast.test.ts`. |
| Names on controls (4.1.2) | Icon-only buttons and switches carry `aria-label`; verified by axe over the rendered components. |
| Keyboard operation (2.1.1) | Every action is a button, a link or a form control — there are no click handlers on plain elements. Global shortcuts are single keys and do not fire while a text field has focus. |
| Hover alternatives (2.1.1, 1.4.13) | The enlarged cover on a subscription hit opens on hover, on keyboard focus and on a tap, and closes on `Escape` or on the next tap. The trigger is a `<button>` with a name and `aria-expanded`, and focus never leaves it, so there is none to give back. A picture that only grows under a pointer is not there at all for somebody using a keyboard or a touchscreen. |
| Drag alternatives (2.1.1, 2.5.7) | Sorting by dragging is duplicated on the keyboard. Every drag handle — LinkGrabber packages and links, download packages and files — is a focusable `<button>` that moves its row one step on `ArrowUp` / `ArrowDown`, and its `aria-label` names both routes. A drag was the only way to reorder until RD-104-05, which put the whole feature out of reach for anyone not using a pointer. |
| Click-in-picture alternatives (2.1.1, 4.1.2) | A click-point captcha is answered by marking a spot in a picture. The picture is a focusable `<button>` with a name and `aria-describedby` pointing at a `role="status"` line that reads the mark's position; the arrow keys move the mark by one pixel, ten with `Shift`, starting from the centre, and `Enter` or `Space` sends it. Verified in `CaptchaDialog.test.ts`, with axe (RD-110-15). |
| Focus in a windowed list (2.4.3, 2.1.1) | Since RD-106-12 the queue and the LinkGrabber render only the rows near the viewport, and a row taken out of the document takes the focus with it — which would have ended the arrow-key reorder above one press in. The row holding focus is therefore pinned and stays rendered wherever the window is, and a keyboard move puts the focus back on the same handle afterwards, so the second press continues the move. Both lists also offer a way back to a selected row, which opens its package first if it is collapsed. |
| Length of a windowed list (1.3.1, 4.1.2) | A list that holds thirty of three thousand rows cannot be counted by a screen reader, so it says its length instead: `role="list"` with a name carrying the total, and `aria-setsize` / `aria-posinset` on every row. |

## How it is checked

```bash
npm run test --prefix web -- src/components/accessibility.test.ts src/assets/contrast.test.ts
```

`axe-core` runs over rendered components and finds the machine-checkable half of WCAG: missing
names, broken roles, duplicate ids, invalid ARIA. Two limits are worth stating rather than
glossing over:

- **axe cannot check contrast in this test run.** Its colour-contrast rule samples rendered
  pixels through a canvas, and jsdom has none. That is why the ratios are computed from the
  palette in a separate test instead.
- **A windowed list is checked in jsdom, which has no layout.** `offsetHeight` is always 0
  there, so the measured row heights never replace the estimated ones and nothing about the
  actual scroll geometry is proved. What the tests do cover is what accessibility depends on:
  that the focused row stays in the document while the window moves past it, that a keyboard
  reorder keeps the focus on its handle past the edge of what is rendered, and that the
  announced length is the list's rather than the window's.
- **The `region` rule is switched off** in the component tests. It asks every piece of content
  to sit inside a landmark, which is a property of the page a component is mounted into — the
  application shell provides the landmarks, and asserting it around an isolated component would
  only be testing the harness.

## What a green run does not prove

A passing automated check is a floor. It cannot tell whether focus lands somewhere sensible
after a dialog closes, whether an announcement says something worth hearing, whether a reading
order makes sense, or whether the interface is usable with a screen reader at all. Those were
checked by hand — keyboard-only navigation through the queue, the LinkGrabber and the settings;
focus return after the confirmation dialog, the captcha dialog and the rename dialog; the
announcement text read back; tabbing to a drag handle in both lists and reordering with the
arrow keys — and they are the parts most likely to regress silently, because nothing here will
fail if they do. The arrow-key reorder now has coverage past the event the handle emits:
`DownloadsView.test.ts` and `LinkGrabberView.test.ts` drive thirty presses through the real view
and assert that the row moved and that the focus is still on the same handle. That the focus
*order* reaches the handle in the first place is still a manual check, and so is everything about
a windowed list that needs real layout.

**Not yet verified:** a run with an actual screen reader (NVDA, VoiceOver, Orca). Until that
has happened, this page claims "targets WCAG 2.2 AA" and not "conforms to it" — the difference
matters to somebody relying on it, and RD-090-11 stays `Partial` for that reason.
