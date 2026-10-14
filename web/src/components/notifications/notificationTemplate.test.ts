/**
 * A rule's own wording filled in the browser for the preview (RD-1280-02): the same rules as
 * `crates/rd-notify/src/template.rs`, which fills the deliveries.
 */
import { describe, expect, it } from 'vitest'

import type { NotificationPlaceholderTable } from '@/api/notificationPlaceholders'
import type { NotificationEvent } from '@/api/types'
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

describe('the traffic budget (RD-1290-02)', () => {
  const BUDGET = ['budget_used', 'budget_left', 'budget_limit', 'budget_period']
  const COMMON = ['event', 'title', ...BUDGET]
  const table: NotificationPlaceholderTable = {
    placeholders: [
      { name: 'event', example: 'package_completed' },
      { name: 'title', example: 'Package finished: Holiday photos' },
      { name: 'budget_used', example: '12.5 GiB' },
      { name: 'budget_left', example: '37.5 GiB' },
      { name: 'budget_limit', example: '50.0 GiB' },
      { name: 'budget_period', example: 'monthly' },
      { name: 'package', example: 'Holiday photos' }
    ],
    events: [
      { event: 'package_completed', takes_template: true, placeholders: [...COMMON, 'package'] },
      { event: 'budget_exhausted', takes_template: true, placeholders: COMMON }
    ]
  }

  it('offers its placeholders with every event that takes a template', () => {
    for (const event of ['package_completed', 'budget_exhausted'] as const) {
      expect(placeholdersFor(table, [event]).names, event).toEqual(expect.arrayContaining(BUDGET))
    }
  })

  it('previews them with the sample values', () => {
    const values = Object.fromEntries(table.placeholders.map(entry => [entry.name, entry.example]))
    const known = new Set(table.placeholders.map(entry => entry.name))
    expect(fillTemplate('{package} done, {budget_left} of {budget_limit} left ({budget_period})', values, known))
      .toEqual({ ok: true, text: 'Holiday photos done, 37.5 GiB of 50.0 GiB left (monthly)' })
  })
})

describe('the queue\'s moments (RD-1290-04)', () => {
  const table: NotificationPlaceholderTable = {
    placeholders: ['event', 'package', 'reason'].map(name => ({ name, example: name })),
    events: [
      { event: 'package_completed', takes_template: true, placeholders: ['event', 'package'] },
      { event: 'queue_paused' as string as NotificationEvent, takes_template: true, placeholders: ['event', 'reason'] }
    ]
  }

  it('offers {reason} for a rule that names the pause', () => {
    expect(placeholdersFor(table, ['queue_paused' as string as NotificationEvent]).names).toEqual(['event', 'reason'])
  })

  it('leaves the pause out of a rule that names no event: it is opt-in', () => {
    expect(placeholdersFor(table, []).names).toEqual(['event', 'package'])
  })
})
