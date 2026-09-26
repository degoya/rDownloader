import axe from 'axe-core'

/**
 * Runs axe over a rendered view and returns its violations as one line each, empty when clean.
 *
 * The view is checked inside a `<main>`, where the application shell mounts every route: a
 * `<header>` there is a section header, not a second banner. `region` is off for the reason
 * `accessibility.test.ts` gives — landmarks are the shell's, not a view's. A test that has to
 * switch off another rule names it in `disabled` and says why beside the call.
 */
export async function axeViolations(container: Element, disabled: string[] = []): Promise<string> {
  const rules = Object.fromEntries(['region', ...disabled].map(id => [id, { enabled: false }]))
  const main = document.createElement('main')
  container.before(main)
  main.append(container)
  try {
    const results = await axe.run(main, { rules })
    return results.violations
      .map(violation => `${violation.id}: ${violation.help} (${violation.nodes.map(node => node.html).join(' | ')})`)
      .join('\n')
  } finally {
    main.before(container)
    main.remove()
  }
}
