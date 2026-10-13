/**
 * A rule's own wording filled in the browser for the preview (RD-1280-02): the same rules as
 * `crates/rd-notify/src/template.rs`, which fills the deliveries.
 */
import { describe, expect, it } from 'vitest'

import type { NotificationPlaceholderTable } from '@/api/notificationPlaceholders'
import { fillTemplate, placeholdersFor } from './notificationTemplate'

const KNOWN = new Set(['package', 'time', 'error', 'size', 'title', 'message'])

describe('fillTemplate', () => {
  it('fills the named placeholders and leaves an unset one empty', () => {
    expect(fillTemplate('{package} at {time}{error}', { package: 'Photos', time: '14:32' }, KNOWN))
      .toEqual({ ok: true, text: 'Photos at 14:32' })
    expect(fillTemplate('{ package }', { package: 'Photos' }, KNOWN)).toEqual({ ok: true, text: 'Photos' })
  })

  it('writes a doubled brace as one and never expands a value again', () => {
    expect(fillTemplate('{{package}} is {package}', { package: 'Photos' }, KNOWN)).toEqual({ ok: true, text: '{package} is Photos' })
    expect(fillTemplate('{{{package}}}', { package: '{error}' }, KNOWN)).toEqual({ ok: true, text: '{{error}}' })
  })

  it('refuses a name that is no placeholder, and a brace out of place', () => {
    expect(fillTemplate('Done: {pakage}', {}, KNOWN)).toEqual({ ok: false, unknown: 'pakage' })
    for (const broken of ['{package', 'package}', '{}', '{ }', '{pack{age}}']) {
      expect(fillTemplate(broken, {}, KNOWN), broken).toEqual({ ok: false, unknown: null })
    }
  })

  it('keeps unicode as it is and turns a value\'s control characters into spaces', () => {
    expect(fillTemplate('\u{1F4E6} {package}', { package: 'Müller\r\nline\u0007' }, KNOWN))
      .toEqual({ ok: true, text: '\u{1F4E6} Müller  line ' })
  })
})

const TABLE: NotificationPlaceholderTable = {
  placeholders: ['event', 'time', 'title', 'package', 'size', 'path', 'error', 'version'].map(name => ({ name, example: name })),
  events: [
    { event: 'package_completed', takes_template: true, placeholders: ['event', 'time', 'title', 'package', 'size', 'path'] },
    { event: 'update_failed', takes_template: true, placeholders: ['event', 'time', 'title', 'version', 'error'] },
    { event: 'download_started', takes_template: false, placeholders: [] }
  ]
}

describe('placeholdersFor', () => {
  it('offers what any chosen event fills, in the table\'s order', () => {
    expect(placeholdersFor(TABLE, ['update_failed', 'package_completed'])).toEqual({
      names: ['event', 'time', 'title', 'package', 'size', 'path', 'error', 'version'],
      summaryOnly: false
    })
    expect(placeholdersFor(TABLE, ['update_failed']).names).toEqual(['event', 'time', 'title', 'error', 'version'])
  })

  it('reads no choice as every event that takes a template', () => {
    expect(placeholdersFor(TABLE, []).names).toContain('package')
    expect(placeholdersFor(TABLE, []).summaryOnly).toBe(false)
  })

  it('says when every chosen event keeps its summary', () => {
    expect(placeholdersFor(TABLE, ['download_started'])).toEqual({ names: [], summaryOnly: true })
  })
})
