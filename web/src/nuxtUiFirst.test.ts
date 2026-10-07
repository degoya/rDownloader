// @vitest-environment node
/**
 * Nuxt UI before anything of our own, held by a ratchet (RD-1110-01).
 *
 * `design.md` (*Nuxt UI before anything of our own*) says a control Nuxt UI offers is taken from
 * Nuxt UI. The audit of 2026-10-05 still found hand-built number fields, empty states, notices,
 * dividers, trees and links all over `web/src`; this counts each of those patterns in every
 * component's template and holds the count to `MAX`. More is a new hand-built control: use the
 * component the pattern names. Fewer is a conversion: lower `MAX` in the same commit, so the
 * number only ever goes down.
 *
 * A hand-built control that stays on purpose goes in `ALLOWED` with the `design.md` passage that
 * explains why, quoted, so a passage that goes away or loses those words fails here instead of leaving an
 * exception nothing carries any more.
 *
 * **What it does not see.** The patterns are regular expressions over the tags of a template, so
 * a control built in a render function or a class list composed in the script is invisible.
 */
import { readFileSync, readdirSync } from 'node:fs'
import { dirname, join, relative, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

import { describe, expect, it } from 'vitest'

const webRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const sourceRoot = join(webRoot, 'src')
const designPath = join(webRoot, '..', 'design.md')

interface Tag {
  file: string
  line: number
  name: string
  /** The tag's attributes as written, line breaks included. */
  attrs: string
  /** The element this tag is the first child of: the opening tag right before it, unless that one closes itself. */
  firstChildOf: string | null
}

interface Pattern {
  /** The Nuxt UI component that replaces the pattern. */
  use: string
  matches: (tag: Tag) => boolean
}

const has = (pattern: RegExp) => (tag: Tag) => pattern.test(tag.attrs)
/** Classes on a Nuxt UI component style that component; only our own markup is hand-built. */
const own = (tag: Tag) => !/^U[A-Z]/.test(tag.name)
const NATIVE_CONTROLS = new Set(['select', 'textarea', 'hr', 'table', 'progress', 'dialog'])
const FRAMES = new Set(['section', 'div', 'form'])
/** The one component that holds a `UInputDate`, its calendar beside it. */
const DATE_FIELD = 'components/DateField.vue'

const PATTERNS = {
  'card': {
    use: 'UCard',
    matches: tag => FRAMES.has(tag.name) && has(/(?<![-:\w])border(?![-\w])/)(tag) && has(/\bborder-(?:muted|default)\b/)(tag)
      && has(/\bbg-(?:default|elevated)\b/)(tag) && has(/(?<![-:\w])p[xy]?-\d/)(tag)
  },
  'number-input': {
    use: 'UInputNumber',
    matches: tag => tag.name === 'UInput' && has(/\btype="number"/)(tag)
  },
  // A time or a date is Nuxt UI's field, never the browser's (owner, 2026-10-06; RD-1120-23).
  'time-date-input': {
    use: 'UInputTime or DateField',
    matches: tag => (tag.name === 'UInput' || tag.name === 'input') && has(/\btype="(?:time|date)"/)(tag)
  },
  // A day is typed or picked from a calendar, which only `DateField` adds to Nuxt UI's date field
  // (RD-1140-09); a bare `UInputDate` is the field without its calendar.
  'bare-date-input': {
    use: 'DateField',
    matches: tag => tag.name === 'UInputDate' && tag.file !== DATE_FIELD
  },
  'dashed-box': {
    use: 'UEmpty',
    matches: tag => own(tag) && has(/\bborder-dashed\b/)(tag) && !has(/@drop\b/)(tag)
  },
  'drop-zone': {
    use: 'UFileUpload',
    matches: tag => own(tag) && has(/\bborder-dashed\b/)(tag) && has(/@drop\b/)(tag)
  },
  'file-input': {
    use: 'UFileUpload',
    matches: tag => tag.name === 'input' && has(/\btype="file"/)(tag)
  },
  'tinted-notice': {
    use: 'UAlert',
    matches: tag => own(tag) && has(/\bbg-(?:success|warning|error|info)\/\d+/)(tag)
  },
  // The folder toggles of a hand-built tree are counted here as well as under `indented-tree`.
  'chevron-toggle': {
    use: 'UCollapsible',
    matches: tag =>
      (tag.name === 'UButton' && has(/'i-lucide-chevron-[a-z]+'\s*:\s*'i-lucide-chevron-/)(tag) && has(/@click\b/)(tag))
      || tag.name === 'details'
  },
  'divider': {
    use: 'USeparator',
    matches: tag => own(tag) && has(/\bborder-t\b/)(tag) && has(/\bpt-\d/)(tag)
  },
  'indented-tree': {
    use: 'UTree',
    matches: tag => own(tag) && has(/paddingLeft:[^"]*depth/)(tag)
  },
  'icon-tile': {
    use: 'UAvatar',
    matches: tag => own(tag) && has(/\bplace-items-center\b/)(tag) && has(/\bsize-(?:8|9|10)\b/)(tag)
  },
  'status-dot': {
    use: 'UChip',
    matches: tag => own(tag) && has(/\bsize-2\b/)(tag) && has(/\bbg-(?:success|warning|error|info|muted)\b/)(tag)
  },
  'raw-link': {
    use: 'ULink',
    matches: tag => tag.name === 'a'
  },
  'raw-button': {
    use: 'UButton',
    matches: tag => tag.name === 'button' || (tag.name === 'component' && has(/:is="[^"]*'button'/)(tag))
  },
  'raw-input': {
    use: 'UInput',
    matches: tag => tag.name === 'input' && !has(/\btype="file"/)(tag)
  },
  'focusable-element': {
    use: 'UButton',
    matches: tag => own(tag) && has(/\btabindex="0"|\brole="(?:button|separator|slider|switch|tab|checkbox)"/)(tag)
  },
  'native-control': {
    use: 'USelect, UTextarea, USeparator, UTable, UProgress or UModal',
    matches: tag => NATIVE_CONTROLS.has(tag.name)
  },
  // A row of buttons whose look follows "is this the chosen one" tells only the eye which value
  // holds; the radio group says it to a screen reader as well (RD-1120-14). Two values are
  // compared, neither a literal: a button coloured for one fixed case is no selection.
  'toggle-group': {
    use: 'URadioGroup or UTabs',
    matches: tag => tag.name === 'UButton' && has(/:variant="\s*[\w$.]+\s*===\s*[\w$.]+\s*\?/)(tag)
  },
  // The empty state of a fetched list is a `UEmpty`, like every other (RD-1120-14).
  'empty-paragraph': {
    use: 'UEmpty',
    matches: tag => tag.name === 'p' && tag.firstChildOf === 'DataState'
  },
  // A unit stands at its number, not at the far end of the label row (RD-1140-08).
  'unit-hint': {
    use: 'NumberWithUnit (a UInputNumber and an outline UBadge in a UFieldGroup)',
    matches: tag => tag.name === 'UFormField' && has(/(?<![:\w-])hint="(?:[KMGTP]?i?B(?:\/s)?|ms|s|min|h|d|%)"/)(tag)
  }
} satisfies Record<string, Pattern>

type PatternId = keyof typeof PATTERNS

/** The ratchet: every count may only go down, and goes down here when it does. */
const MAX: Record<PatternId, number> = {
  'card': 0,
  'number-input': 0,
  'time-date-input': 0,
  'bare-date-input': 0,
  'dashed-box': 0,
  'drop-zone': 0,
  'file-input': 0,
  'tinted-notice': 0,
  'chevron-toggle': 0,
  'divider': 0,
  'indented-tree': 0,
  'icon-tile': 0,
  'status-dot': 0,
  'raw-link': 0,
  'raw-button': 0,
  'raw-input': 0,
  'focusable-element': 0,
  'native-control': 0,
  'toggle-group': 0,
  'empty-paragraph': 0,
  'unit-hint': 0
}

interface Allowance {
  /** Relative to `web/src`. */
  file: string
  pattern: PatternId
  /** Exactly this many hits of the pattern in the file are exempt; a new one is counted. */
  count: number
  /** Only hits whose attributes match, where the file has other hits that do count. */
  where?: RegExp
  /** `design.md:<line>` or `design.md:<from>-<to>`, the passage that makes the hand-built control the right one. */
  design: string
  /** Words from that passage; the test fails when `design.md` no longer holds them. */
  quote: string
}

const ALLOWED: Allowance[] = [
  // A framed box inside a card or a modal, or a list row, is not a card of its own.
  { file: 'components/CaptchaDialog.vue', pattern: 'card', count: 1, design: 'design.md:402-404', quote: 'inner boxes of a card or a modal (previews, code samples, the statistics chart)' },
  { file: 'components/QueueSummary.vue', pattern: 'card', count: 1, where: /props\.summary\.storage/, design: 'design.md:402-404', quote: 'inner boxes of a card or a modal (previews, code samples, the statistics chart)' },
  { file: 'components/settings/AccountBrowserSession.vue', pattern: 'card', count: 1, design: 'design.md:402-404', quote: 'inner boxes of a card or a modal (previews, code samples, the statistics chart)' },
  { file: 'components/settings/AccountSignInFlow.vue', pattern: 'card', count: 1, design: 'design.md:402-404', quote: 'inner boxes of a card or a modal (previews, code samples, the statistics chart)' },
  { file: 'components/settings/PluginInstallPreviewModal.vue', pattern: 'card', count: 1, design: 'design.md:402-404', quote: 'inner boxes of a card or a modal (previews, code samples, the statistics chart)' },
  { file: 'components/settings/RemoteJobSubmitForm.vue', pattern: 'card', count: 1, design: 'design.md:402-404', quote: 'inner boxes of a card or a modal (previews, code samples, the statistics chart)' },
  { file: 'components/settings/SettingsLinkgrabberTab.vue', pattern: 'card', count: 1, design: 'design.md:402-404', quote: 'inner boxes of a card or a modal (previews, code samples, the statistics chart)' },
  { file: 'components/settings/SettingsPluginRepositories.vue', pattern: 'card', count: 1, design: 'design.md:402-404', quote: 'inner boxes of a card or a modal (previews, code samples, the statistics chart)' },
  // The drag handle of a reorderable row, one component for every list that reorders (RD-1120-14).
  { file: 'components/DragHandle.vue', pattern: 'raw-button', count: 1, design: 'design.md:1446-1450', quote: 'it is a `<button>` carrying `draggable="true"`' },
  // The expand chevron of a queue or LinkGrabber row is a grid cell of its own.
  { file: 'components/TransferCard.vue', pattern: 'chevron-toggle', count: 1, design: 'design.md:1617-1619', quote: 'not the trigger of a `UCollapsible`' },
  { file: 'components/PackageGroup.vue', pattern: 'chevron-toggle', count: 1, design: 'design.md:1617-1619', quote: 'not the trigger of a `UCollapsible`' },
  { file: 'components/CollectorCandidateRow.vue', pattern: 'chevron-toggle', count: 2, design: 'design.md:1617-1619', quote: 'not the trigger of a `UCollapsible`' },
  // A LinkGrabber package opens by filtering the windowed stream, not by a `v-if` around its links.
  { file: 'components/CollectorPackageGroup.vue', pattern: 'chevron-toggle', count: 1, design: 'design.md:1351-1352', quote: 'collapsing is a filter on that stream, not a `v-if` inside a package' },
  // A LinkGrabber group whose header carries its own actions and opens a block of its own under it.
  { file: 'components/NzbImportGroup.vue', pattern: 'chevron-toggle', count: 1, design: 'design.md:1711-1714', quote: 'a collapsible would take the header\'s actions into its trigger' },
  { file: 'components/IndexerReviewGroup.vue', pattern: 'chevron-toggle', count: 1, design: 'design.md:1711-1714', quote: 'a collapsible would take the header\'s actions into its trigger' },
  { file: 'components/SubscriptionItemRow.vue', pattern: 'raw-button', count: 1, design: 'design.md:1229-1230', quote: 'The trigger is a `<button>` with a name and `aria-expanded`' },
  { file: 'components/CaptchaDialog.vue', pattern: 'raw-button', count: 1, design: 'design.md:688-689', quote: 'picture is a `<button>` with a name' },
  { file: 'components/SubscriptionItemSlider.vue', pattern: 'raw-button', count: 1, design: 'design.md:1316-1317', quote: 'why it is no `UCarousel`' },
  { file: 'components/SubscriptionItemSlider.vue', pattern: 'focusable-element', count: 1, design: 'design.md:1340', quote: 'The track is focusable and turns pages with the arrow keys' },
  // The regex diagram: a group is a labelled frame of the drawing, not an empty state; the frame
  // that scrolls a wide pattern sideways takes keyboard focus (axe scrollable-region-focusable).
  { file: 'components/routing/RegexDiagramNode.vue', pattern: 'dashed-box', count: 1, design: 'design.md:1888', quote: 'A group is a dashed primary frame' },
  { file: 'components/routing/RegexDiagram.vue', pattern: 'focusable-element', count: 1, design: 'design.md:1892', quote: 'keyboard focusable' },
  { file: 'components/QueueColumnHeader.vue', pattern: 'focusable-element', count: 1, design: 'design.md:1632-1633', quote: 'has no handle for a grid that is not a `UTable`; this one is the exception' },
  { file: 'components/ControlRoomLayout.vue', pattern: 'raw-link', count: 1, design: 'design.md:377-379', quote: 'not a `ULink`, because it is an in-page jump' },
  { file: 'components/NzbDropOverlay.vue', pattern: 'dashed-box', count: 1, design: 'design.md:375-377', quote: 'The overlay is a picture of the drop over the whole page, not a field' },
  // A file tree's rows hold checkboxes and priority selects, which a `UTree` row cannot (RD-1110-12).
  { file: 'components/RemoteFileTree.vue', pattern: 'indented-tree', count: 1, design: 'design.md:1677-1679', quote: 'build their rows themselves — indented by depth, a chevron button per folder' },
  { file: 'components/RemoteFileTree.vue', pattern: 'chevron-toggle', count: 1, design: 'design.md:1677-1679', quote: 'build their rows themselves — indented by depth, a chevron button per folder' },
  { file: 'components/TorrentFileTree.vue', pattern: 'indented-tree', count: 1, design: 'design.md:1677-1679', quote: 'build their rows themselves — indented by depth, a chevron button per folder' },
  { file: 'components/TorrentFileTree.vue', pattern: 'chevron-toggle', count: 1, design: 'design.md:1677-1679', quote: 'build their rows themselves — indented by depth, a chevron button per folder' }
]

const TAG = /<([a-zA-Z][\w-]*)((?:\s+[^\s"'=<>/]+(?:\s*=\s*(?:"[^"]*"|'[^']*'|[^\s>]+))?)*)\s*\/?>/g

/** Every opening tag of a single-file component's template; comments are blanked, lines kept. */
function templateTags(file: string, source: string): Tag[] {
  const start = source.search(/^<template[\s>]/m)
  const end = source.lastIndexOf('</template>')
  if (start < 0 || end < start) {
    return []
  }
  const template = source.slice(start, end).replace(/<!--[\s\S]*?-->/g, comment => comment.replace(/[^\n]/g, ' '))
  const before = source.slice(0, start).split('\n').length - 1
  const matches = [...template.matchAll(TAG)]
  return matches.map((match, index) => {
    const previous = matches[index - 1]
    return {
      file,
      line: before + template.slice(0, match.index).split('\n').length,
      name: match[1] ?? '',
      attrs: match[2] ?? '',
      firstChildOf: previous && !previous[0].endsWith('/>') ? previous[1] ?? null : null
    }
  })
}

function components(directory: string): string[] {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = join(directory, entry.name)
    if (entry.isDirectory()) {
      return components(path)
    }
    return entry.name.endsWith('.vue') ? [path] : []
  })
}

type Hits = Record<PatternId, Tag[]>

function hits(tags: Tag[]): Hits {
  const found = Object.fromEntries(Object.keys(PATTERNS).map(id => [id, [] as Tag[]])) as Hits
  for (const tag of tags) {
    for (const [id, pattern] of Object.entries(PATTERNS) as [PatternId, Pattern][]) {
      if (pattern.matches(tag)) {
        found[id].push(tag)
      }
    }
  }
  return found
}

/** The hits `ALLOWED` does not cover, and every allowance that covers fewer hits than it names. */
function counted(id: PatternId, found: Tag[], allowed: Allowance[]): { tags: Tag[], stale: string[] } {
  const stale: string[] = []
  let tags = found
  for (const allowance of allowed.filter(entry => entry.pattern === id)) {
    const inFile = tags.filter(tag => tag.file === allowance.file && (allowance.where?.test(tag.attrs) ?? true))
    if (inFile.length < allowance.count) {
      stale.push(`${allowance.file}: allows ${allowance.count} ${id}, finds ${inFile.length}; lower or remove the allowance`)
    }
    const exempt = new Set(inFile.slice(0, allowance.count))
    tags = tags.filter(tag => !exempt.has(tag))
  }
  return { tags, stale }
}

/** Empty when the count equals `MAX`; otherwise what to do about it. */
function verdict(id: PatternId, tags: Tag[], max: number): string {
  if (tags.length > max) {
    const where = tags.map(tag => `  ${tag.file}:${tag.line} <${tag.name}>`).join('\n')
    return `${id}: ${tags.length} hand-built, MAX ${max}. Use ${PATTERNS[id].use}; the hits:\n${where}`
  }
  if (tags.length < max) {
    return `${id}: ${tags.length} left, MAX ${max}. Lower MAX['${id}'] to ${tags.length} in nuxtUiFirst.test.ts.`
  }
  return ''
}

const sourceHits = hits(components(sourceRoot).flatMap(path =>
  templateTags(relative(sourceRoot, path), readFileSync(path, 'utf8'))))

describe('Nuxt UI first', () => {
  it.each(Object.keys(PATTERNS) as PatternId[])('holds %s to its MAX', (id) => {
    const { tags, stale } = counted(id, sourceHits[id], ALLOWED)
    expect(stale).toEqual([])
    expect(verdict(id, tags, MAX[id])).toBe('')
  })

  it('names a design.md passage that exists for every allowance', () => {
    // The quote is looked up in the whole file: line numbers move whenever two branches add a
    // section (the 1.11 wave 2 merge), the passage's words do not. `design` stays the reader's
    // pointer and only has to have the form `design.md:<line>[-<line>]`.
    const design = readFileSync(designPath, 'utf8').replace(/\s+/g, ' ')
    const missing = ALLOWED.filter((allowance) => {
      if (!/^design\.md:\d+(?:-\d+)?$/.test(allowance.design)) {
        return true
      }
      return !design.includes(allowance.quote)
    })
    expect(missing.map(allowance => `${allowance.file} ${allowance.pattern}: ${allowance.design} lacks "${allowance.quote}"`)).toEqual([])
  })

  describe('the guard itself', () => {
    const fixture = (template: string) => hits(templateTags('Fixture.vue', `<script setup lang="ts">\n</script>\n\n<template>\n${template}\n</template>\n`))

    it('turns red on a new hand-built control, naming file and line', () => {
      const found = fixture('  <div>\n    <hr>\n    <button\n      type="button"\n      @click="go"\n    >Go</button>\n  </div>')
      expect(verdict('native-control', found['native-control'], 0)).toContain('Fixture.vue:6 <hr>')
      expect(verdict('raw-button', found['raw-button'], 0)).toContain('Fixture.vue:7 <button>')
    })

    it('asks for a lower MAX once a hand-built control is gone', () => {
      const found = fixture('  <UInputNumber v-model="limit" />')
      expect(verdict('number-input', found['number-input'], 1)).toContain(`Lower MAX['number-input'] to 0`)
    })

    it('turns red on a time or a date field of the browser', () => {
      const found = fixture('  <UInput v-model="start" type="time" />\n  <UInput v-model="day" type="date" />\n  <UInputTime v-model="start" />')
      expect(found['time-date-input'].map(tag => tag.line)).toEqual([5, 6])
    })

    it('turns red on a date field without its calendar, anywhere but in DateField', () => {
      const template = '  <UInputDate v-model="day" />\n  <DateField v-model="day" />'
      expect(fixture(template)['bare-date-input'].map(tag => tag.line)).toEqual([5])
      const inside = hits(templateTags(DATE_FIELD, `<template>\n${template}\n</template>\n`))
      expect(inside['bare-date-input']).toEqual([])
    })

    it('tells a selection row from a button coloured for a fixed case', () => {
      const found = fixture('  <UButton v-for="item in items" :key="item" :variant="mode === item ? \'soft\' : \'ghost\'" />\n  <UButton :variant="decision === \'rename\' ? \'solid\' : \'outline\'" />')
      expect(found['toggle-group'].map(tag => tag.line)).toEqual([5])
    })

    it('finds a paragraph standing in for the empty state of a DataState, and only there', () => {
      const found = fixture('  <DataState :empty="true">\n    <p class="text-sm text-muted">Nothing</p>\n  </DataState>\n  <DataState :empty="true" />\n  <p class="text-sm text-muted">Below</p>')
      expect(found['empty-paragraph'].map(tag => tag.line)).toEqual([6])
    })

    it('turns red on a unit as the hint of a field, not on a sentence', () => {
      const found = fixture('  <UFormField hint="MiB/s" label="Limit">\n    <UInputNumber v-model="limit" />\n  </UFormField>\n  <UFormField\n    hint="s"\n    label="Timeout"\n  />\n  <UFormField :hint="t(\'rule\')" />\n  <UFormField hint="Optional" />')
      expect(found['unit-hint'].map(tag => tag.line)).toEqual([5, 8])
    })

    it('ignores a control that is only mentioned in a comment', () => {
      const found = fixture('  <!-- once a <button> and an <a href="#"> -->\n  <UButton label="Go" />')
      expect(found['raw-button']).toEqual([])
      expect(found['raw-link']).toEqual([])
    })

    it('counts what an allowance does not cover, and reports an allowance with nothing left', () => {
      const found = fixture('  <button type="button" title="One">One</button>\n  <button type="button" title="Two">Two</button>')['raw-button']
      const allowance: Allowance = { file: 'Fixture.vue', pattern: 'raw-button', count: 1, design: 'design.md:1-1', quote: '' }
      expect(counted('raw-button', found, [allowance]).tags.map(tag => tag.line)).toEqual([6])
      expect(counted('raw-button', [], [allowance]).stale).toHaveLength(1)
      const second = { ...allowance, where: /title="Two"/ }
      expect(counted('raw-button', found, [second]).tags.map(tag => tag.line)).toEqual([5])
    })
  })
})
