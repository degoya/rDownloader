/**
 * A rule's own wording filled in the browser for the preview (RD-1280-02): the same rules as
 * `crates/rd-notify/src/template.rs`, which fills the deliveries.
 */
import { describe, expect, it } from 'vitest'

import type { NotificationPlaceholderTable } from '@/api/notificationPlaceholders'
import type { NotificationEvent } from '@/api/types'
import { fillTemplate, placeholdersFor, samplesFor } from './notificationTemplate'

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
  placeholders: ['event', 'time', 'title', 'package', 'size', 'path', 'error', 'version', 'count', 'names', 'file']
    .map(name => ({ name, example: name })),
  events: [
    { event: 'package_completed', takes_template: true, placeholders: ['event', 'time', 'title', 'package', 'size', 'path'], in_bursts: false, examples: [] },
    { event: 'update_failed', takes_template: true, placeholders: ['event', 'time', 'title', 'version', 'error'], in_bursts: false, examples: [] },
    {
      event: 'download_started',
      takes_template: true,
      placeholders: ['event', 'time', 'title', 'count', 'names', 'file', 'package'],
      in_bursts: true,
      examples: [
        { name: 'event', example: 'download_started' },
        { name: 'title', example: '3 downloads started' },
        { name: 'count', example: '3' },
        { name: 'names', example: 'a.mkv, b.mkv, c.mkv' },
        { name: 'file', example: '' },
        { name: 'package', example: '' }
      ]
    }
  ]
}

describe('placeholdersFor', () => {
  it('offers what any chosen event fills, in the table\'s order', () => {
    expect(placeholdersFor(TABLE, ['update_failed', 'package_completed'])).toEqual({
      names: ['event', 'time', 'title', 'package', 'size', 'path', 'error', 'version'],
      bursts: false
    })
    expect(placeholdersFor(TABLE, ['update_failed']).names).toEqual(['event', 'time', 'title', 'error', 'version'])
  })

  it('reads no choice as every event that takes a template', () => {
    expect(placeholdersFor(TABLE, []).names).toContain('package')
    expect(placeholdersFor(TABLE, []).names).not.toContain('count')
    expect(placeholdersFor(TABLE, []).bursts).toBe(false)
  })

  it('offers a burst its own placeholders and says that it arrives in bursts (RD-1291-01)', () => {
    expect(placeholdersFor(TABLE, ['download_started'])).toEqual({
      names: ['event', 'time', 'title', 'package', 'count', 'names', 'file'],
      bursts: true
    })
    expect(placeholdersFor(TABLE, ['download_started', 'update_failed']).bursts).toBe(true)
  })
})

describe('samplesFor (RD-1291-01)', () => {
  it('previews a rule for bursts alone with the sample burst, a file left empty', () => {
    const names = placeholdersFor(TABLE, ['download_started']).names
    const samples = samplesFor(TABLE, ['download_started'], names)
    expect(samples).toMatchObject({ event: 'download_started', title: '3 downloads started', count: '3', file: '', package: '', time: 'time' })
    const known = new Set(TABLE.placeholders.map(entry => entry.name))
    expect(fillTemplate('{count} started: {names}[{file}]', samples, known))
      .toEqual({ ok: true, text: '3 started: a.mkv, b.mkv, c.mkv[]' })
  })

  it('keeps the table\'s samples when other events are chosen too, and offers only what they fill', () => {
    const names = placeholdersFor(TABLE, ['package_completed', 'download_started']).names
    const samples = samplesFor(TABLE, ['package_completed', 'download_started'], names)
    expect(samples.title).toBe('title')
    expect(samples.package).toBe('package')
    expect(samplesFor(TABLE, ['update_failed'], ['event', 'version'])).toEqual({ event: 'event', version: 'version' })
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
      { event: 'package_completed', takes_template: true, placeholders: [...COMMON, 'package'], in_bursts: false, examples: [] },
      { event: 'budget_exhausted', takes_template: true, placeholders: COMMON, in_bursts: false, examples: [] }
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
      { event: 'package_completed', takes_template: true, placeholders: ['event', 'package'], in_bursts: false, examples: [] },
      {
        event: 'queue_paused' as string as NotificationEvent,
        takes_template: true,
        placeholders: ['event', 'reason'],
        in_bursts: false,
        examples: []
      }
    ]
  }

  it('offers {reason} for a rule that names the pause', () => {
    expect(placeholdersFor(table, ['queue_paused' as string as NotificationEvent]).names).toEqual(['event', 'reason'])
  })

  it('leaves the pause out of a rule that names no event: it is opt-in', () => {
    expect(placeholdersFor(table, []).names).toEqual(['event', 'package'])
  })
})
