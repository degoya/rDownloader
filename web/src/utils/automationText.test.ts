import { afterEach, beforeAll, describe, expect, it } from 'vitest'

import type { AutomationVersion } from '@/api/types'
import { i18n, setLocale } from '@/i18n'
import { loadEveryLocale } from '@/test/locales'

import { changedParts, describeAction, describeCondition, describeTrigger } from './automationText'

const t = (key: string, params?: Record<string, unknown>) => {
  if (key === 'common.text.labelled') return `${String(params?.label)}: ${String(params?.value)}`
  if (key === 'common.text.quoted') return `“${String(params?.text)}”`
  return params ? `<${key} ${JSON.stringify(params)}>` : `<${key}>`
}

function version(number: number, part: Partial<AutomationVersion>): AutomationVersion {
  return {
    id: `v${number}`,
    automation_id: 'a1',
    version: number,
    trigger: 'download_completed',
    condition: { type: 'always' },
    actions: [{ kind: 'pause_package' }],
    created_at: '2026-10-02T10:00:00Z',
    ...part
  }
}

describe('describeCondition', () => {
  it('reads a nested tree in reading order', () => {
    expect(describeCondition({
      type: 'all',
      nodes: [
        { type: 'predicate', predicate: { field: 'name', operator: 'contains', value: 'x' } },
        { type: 'not', node: { type: 'predicate', predicate: { field: 'extension', operator: 'equals', value: 'nfo' } } }
      ]
    }, t)).toBe('<automation.condition.all> (<automation.condition.comparison {"field":"<automation.field.name>","operator":"<automation.operator.contains>","value":"x"}>; '
      + '<automation.condition.not> (<automation.condition.comparison {"field":"<automation.field.extension>","operator":"<automation.operator.equals>","value":"nfo"}>))')
    expect(describeCondition({ type: 'always' }, t)).toBe('<automation.condition.always>')
  })
})

describe('describeAction', () => {
  it('names what an action points at, or its id when that is gone', () => {
    const names = (id: string) => (id === 'c1' ? 'Movies' : undefined)
    expect(describeAction({ kind: 'set_category', category_id: 'c1' }, t, names)).toBe('<automation.action.set_category>: Movies')
    expect(describeAction({ kind: 'webhook', target_id: 'gone' }, t, names)).toBe('<automation.action.webhook>: gone')
    expect(describeAction({ kind: 'script', name: 'tidy.sh' }, t, names)).toBe('<automation.action.script>: tidy.sh')
    expect(describeAction({ kind: 'pause_package' }, t, names)).toBe('<automation.action.pause_package>')
  })

  it('says what the actions of RD-1240-10 carry', () => {
    const names = (id: string) => (id === 't1' ? 'Ops' : undefined)
    expect(describeAction({ kind: 'set_priority', priority: 'high' }, t, names))
      .toBe('<automation.action.set_priority>: <automation.action.priority_value.high>')
    expect(describeAction({ kind: 'notify', target_id: 't1', message: 'Started' }, t, names))
      .toBe('<automation.action.notify>: Ops “Started”')
    expect(describeAction({ kind: 'add_links', links: ['https://a.example/x', 'https://b.example/y'], destination: 'downloads' }, t, names))
      .toBe('<automation.action.add_links>: <automation.action.link_count {"count":2}> → <automation.action.destination_value.downloads>')
    expect(describeAction({ kind: 'start_queue' }, t, names)).toBe('<automation.action.start_queue>')
  })
})

describe('the quotes and colons of a sentence', () => {
  beforeAll(loadEveryLocale)
  afterEach(() => setLocale('en'))

  it.each([
    ['en', '“x”', /\w: tidy\.sh$/],
    ['de', '„x“', /\w: tidy\.sh$/],
    ['es', '«x»', /\w: tidy\.sh$/],
    ['fr', '«\u00a0x\u00a0»', /\w\s: tidy\.sh$/]
  ])('are those of %s', async (locale, quoted, action) => {
    await setLocale(locale)
    const translate = i18n.global.t as (key: string, params?: Record<string, unknown>) => string
    expect(describeCondition({ type: 'predicate', predicate: { field: 'name', operator: 'contains', value: 'x' } }, translate)).toMatch(new RegExp(`${quoted}$`))
    expect(describeAction({ kind: 'script', name: 'tidy.sh' }, translate, () => undefined)).toMatch(action)
  })
})

describe('describeTrigger', () => {
  it('says when a time trigger runs', () => {
    expect(describeTrigger('schedule', { kind: 'interval', minutes: 30 }, t))
      .toBe('<automation.trigger.schedule>: <automation.schedule.every_minutes {"minutes":30}>')
    expect(describeTrigger('schedule', { kind: 'cron', expression: '0 6 * * 1-5' }, t))
      .toBe('<automation.trigger.schedule>: 0 6 * * 1-5')
    expect(describeTrigger('package_completed', null, t)).toBe('<automation.trigger.package_completed>')
  })
})

describe('changedParts', () => {
  it('marks what a version changed against the one before it', () => {
    const first = version(1, {})
    const second = version(2, { actions: [{ kind: 'resume_package' }] })
    expect([...changedParts(first, undefined)]).toEqual([])
    expect([...changedParts(second, first)]).toEqual(['actions'])
    expect([...changedParts(version(3, { trigger: 'package_failed', condition: { type: 'not', node: { type: 'always' } } }), second)])
      .toEqual(['trigger', 'condition', 'actions'])
  })

  it('counts a changed schedule as a changed trigger', () => {
    const hourly = version(1, { trigger: 'schedule', schedule: { kind: 'interval', minutes: 60 } })
    const daily = version(2, { trigger: 'schedule', schedule: { kind: 'cron', expression: '0 6 * * *' } })
    expect([...changedParts(daily, hourly)]).toEqual(['trigger'])
  })
})
