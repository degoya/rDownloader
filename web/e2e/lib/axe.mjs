// axe over a whole page of a real browser (RD-1480-25): what jsdom cannot measure — contrast of
// what is painted, size, what is hidden by layout — on top of what `src/test/axe.ts` checks on
// single components.
import axe from 'axe-core'

/** WCAG 2.2 A and AA, the level `docs/accessibility.md` names. axe's own "best practices" are not. */
const TAGS = ['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa']

/**
 * Findings that come from a library and cannot be fixed here. Each names one rule and the one
 * element it may be about, so a second element breaking the same rule is still reported.
 */
export const KNOWN = [
  {
    rule: 'aria-allowed-attr',
    selector: 'span[id^="reka-accordion-trigger-"]',
    reason: 'Nuxt UI 4 renders the trailing trigger of a vertical navigation menu as a <span> (NavigationMenu.vue, `as: "span"`), which takes no aria-expanded; the link beside it carries the state'
  }
]

/**
 * Runs axe over `page` in the colour mode it shows and returns its violations as one line each,
 * empty when clean. `page.evaluate` is not subject to the page's Content-Security-Policy, which
 * refuses an injected script.
 */
export async function axeViolations(page) {
  await page.evaluate(axe.source)
  const violations = await page.evaluate(
    (tags) => window.axe.run(document, { runOnly: { type: 'tag', values: tags }, resultTypes: ['violations'] }).then((results) => results.violations),
    TAGS
  )
  return violations.flatMap((violation) => {
    const known = KNOWN.filter((entry) => entry.rule === violation.id)
    return violation.nodes
      .filter((node) => !known.some((entry) => isKnown(node, entry)))
      .map((node) => `${violation.id}: ${violation.help} — ${node.html.slice(0, 200)} (${node.any[0]?.message ?? node.all[0]?.message ?? ''})`)
  })
}

/** Whether `node` is the element a known finding names: `tag[id^="prefix"]`, read off its HTML. */
function isKnown(node, { selector }) {
  const [, tag, prefix] = selector.match(/^(\w+)\[id\^="([^"]+)"\]$/)
  return node.html.startsWith(`<${tag} `) && node.html.includes(` id="${prefix}`)
}

/** Waits until no transition or animation runs: a colour read halfway between two is nobody's. */
export function settled(page) {
  return page.waitForFunction(() => document.getAnimations().every((animation) => animation.playState !== 'running'))
}
