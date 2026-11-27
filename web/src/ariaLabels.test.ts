/**
 * An `aria-label` reaches a screen reader only on an element that may carry one (RD-1490-09).
 *
 * ARIA prohibits a name on a `<span>` or `<div>` without a role: a screen reader skips it and
 * axe reports it. A label on text or a glyph goes with `role="img"`, as the connection count in
 * the queue does. A `UIcon` is drawn by Iconify, which marks every icon `aria-hidden="true"`
 * unless it is told `aria-hidden="false"`, so a labelled icon needs both, or its label is hidden
 * with it.
 *
 * **What it does not see.** The scan reads the tags of a template with a regular expression, so a
 * label bound through `v-bind="…"` or set in a render function passes unseen.
 */
import { readdirSync, readFileSync } from 'node:fs'
import { dirname, join, relative, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

import { addIcon } from '@iconify/vue'
import Icon from '@nuxt/ui/runtime/vue/components/Icon.vue'
import { render, screen } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'

const sourceRoot = resolve(dirname(fileURLToPath(import.meta.url)))
/** Elements without a role of their own; `section` is a region once it has a name, so not here. */
const GENERIC = new Set(['span', 'div', 'p', 'small', 'strong', 'em', 'b', 'i', 'time'])
const LABEL = /(?:^|\s):?aria-label(?:ledby)?=/
const ROLE = /(?:^|\s):?role=/

function vueFiles(directory: string): string[] {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = join(directory, entry.name)
    if (entry.isDirectory()) return vueFiles(path)
    return entry.name.endsWith('.vue') ? [path] : []
  })
}

/** Every opening tag of the template that carries a label, as `file:line <tag attrs>`. */
function labelledTags(): { where: string, name: string, attrs: string }[] {
  return vueFiles(sourceRoot).flatMap((file) => {
    const source = readFileSync(file, 'utf8')
    const start = source.indexOf('<template')
    const tags = [...source.slice(start).matchAll(/<([A-Za-z][\w-]*)((?:[^>"']|"[^"]*"|'[^']*')*)>/g)]
    return tags
      .filter(([, , attrs]) => LABEL.test(attrs ?? ''))
      .map((tag) => {
        const line = source.slice(0, start + tag.index).split('\n').length
        return { where: `${relative(sourceRoot, file)}:${line}`, name: tag[1]!, attrs: tag[2]! }
      })
  })
}

describe('labels in the templates', () => {
  const tags = labelledTags()

  it('finds the labels it checks', () => {
    expect(tags.length).toBeGreaterThan(100)
  })

  it('names no element that has no role', () => {
    const bare = tags.filter(tag => GENERIC.has(tag.name) && !ROLE.test(tag.attrs)).map(tag => `${tag.where} <${tag.name}>`)
    expect(bare).toEqual([])
  })

  it('shows a labelled icon to a screen reader', () => {
    const hidden = tags
      .filter(tag => tag.name === 'UIcon' && !(/\srole="img"/.test(tag.attrs) && /\saria-hidden="false"/.test(tag.attrs)))
      .map(tag => tag.where)
    expect(hidden).toEqual([])
  })
})

describe('a labelled icon', () => {
  addIcon('lucide-lock', { body: '<path d="M0 0h24v24H0z"/>', width: 24, height: 24 })

  it('is an image with its label once it is no longer hidden', () => {
    render(Icon, { props: { name: 'i-lucide-lock' }, attrs: { 'role': 'img', 'aria-hidden': 'false', 'aria-label': 'Locked' } })
    expect(screen.getByRole('img', { name: 'Locked' })).toBeTruthy()
  })

  it('is hidden, label and all, by Iconify\'s default', () => {
    render(Icon, { props: { name: 'i-lucide-lock' }, attrs: { 'role': 'img', 'aria-label': 'Locked' } })
    expect(screen.queryByRole('img', { name: 'Locked' })).toBeNull()
  })
})
