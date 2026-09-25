/**
 * The translation between the editor's named fields and the rule body the service parses
 * (RD-110-08).
 *
 * This is the only place in the frontend that knows the shape of a rule, so it is the only
 * place that can get it wrong silently: the service refuses an unknown field outright
 * (`deny_unknown_fields`), and an optional field written as an empty string is not the same
 * as an absent one.
 */
import { describe, expect, it } from 'vitest'

import type { SiteRule } from '@/api/types'
import {
  STEP_KINDS,
  copyBody,
  copyId,
  draftComplete,
  emptyDraft,
  emptyStep,
  fromRule,
  toBody,
  type StepKind
} from './useSiteRules'

function filled() {
  const draft = emptyDraft()
  draft.id = 'my-board'
  draft.name = 'My board'
  draft.group = 'board'
  draft.hosts = 'example.org\n*.example.org\n'
  draft.paths = '^/release/'
  draft.probe = 'https://example.org/release/1'
  draft.checked = '2026-09-22'
  draft.steps = [emptyStep('fetch'), { ...emptyStep('regex'), pattern: 'href="([^"]+)"', into: 'links', all: true }]
  return draft
}

describe('the rule body a draft describes', () => {
  it('leaves out every optional field the person left empty', () => {
    const body = toBody(filled()) as Record<string, unknown>
    expect(body.match).toEqual({ hosts: ['example.org', '*.example.org'], paths: ['^/release/'] })
    expect('dead' in body).toBe(false)
    expect('mirrors' in body).toBe(false)
    const steps = body.steps as Record<string, unknown>[]
    // A `fetch` with neither an address nor a target carries neither key, rather than two
    // empty strings the service would read as a variable named "".
    expect(steps[0]).toEqual({ kind: 'fetch' })
    expect(steps[1]).toEqual({ kind: 'regex', pattern: 'href="([^"]+)"', into: 'links', all: true })
    expect(body.package).toEqual({ from: 'title' })
  })

  it('writes the fields of every one of the seven kinds under its own name', () => {
    const draft = filled()
    draft.steps = STEP_KINDS.map((kind: StepKind) => ({
      ...emptyStep(kind),
      url: 'https://example.org/x',
      into: 'value',
      from: 'page',
      path: '/items/0',
      pattern: 'x',
      fields: 'go=1\nname = two ',
      challenge: 'recaptcha-v2',
      sitekey: '${key}'
    }))
    const steps = toBody(draft).steps as Record<string, unknown>[]
    expect(steps.map(step => step.kind)).toEqual([...STEP_KINDS])
    expect(steps[1]).toEqual({ kind: 'fetch-json', url: 'https://example.org/x', path: '/items/0', into: 'value' })
    expect(steps[3]).toEqual({ kind: 'decode', encoding: 'base64', from: 'page', into: 'value' })
    // The form's `name=value` lines become an object, and the spaces around them go.
    expect(steps[4]).toEqual({
      kind: 'form',
      url: 'https://example.org/x',
      fields: { go: '1', name: 'two' },
      into: 'value'
    })
    expect(steps[5]).toEqual({ kind: 'redirect', from: 'page', into: 'value' })
    expect(steps[6]).toEqual({ kind: 'captcha', challenge: 'recaptcha-v2', sitekey: '${key}', into: 'value' })
  })

  it('carries a package source that is not the page title', () => {
    const draft = filled()
    draft.packageFrom = 'regex'
    draft.packagePattern = '<h1>(.*?)</h1>'
    expect(toBody(draft).package).toEqual({ from: 'regex', pattern: '<h1>(.*?)</h1>' })
    draft.packageSource = 'container'
    expect(toBody(draft).package).toEqual({ from: 'regex', pattern: '<h1>(.*?)</h1>', source: 'container' })
    draft.packageFrom = 'variable'
    draft.packageName = 'title'
    expect(toBody(draft).package).toEqual({ from: 'variable', name: 'title' })
  })

  it('opens a stored rule back into the same draft', () => {
    const draft = filled()
    draft.dead = 'old.example.org'
    draft.mirrors = true
    const rule: SiteRule = {
      id: draft.id,
      name: draft.name,
      group: draft.group,
      hosts: ['example.org', '*.example.org'],
      version: 1,
      probe: draft.probe,
      mirrors: true,
      steps: 2,
      enabled: true,
      active: true,
      rule: toBody(draft) as never,
      check: null
    }
    const reopened = fromRule(rule)
    expect(toBody(reopened)).toEqual(toBody(draft))
    expect(reopened.hosts).toBe('example.org\n*.example.org')
    expect(reopened.enabled).toBe(true)
  })

  it('refuses to call a draft complete before the service would look at it', () => {
    expect(draftComplete(emptyDraft())).toBe(false)
    const draft = filled()
    expect(draftComplete(draft)).toBe(true)
    expect(draftComplete({ ...draft, hosts: '  \n ' })).toBe(false)
    expect(draftComplete({ ...draft, steps: [] })).toBe(false)
    expect(draftComplete({ ...draft, probe: '' })).toBe(false)
  })
})

describe('a copy of a rule (RD-130-07)', () => {
  it('takes the first free identifier after the original', () => {
    expect(copyId('scnlog', ['scnlog'])).toBe('scnlog-copy')
    expect(copyId('scnlog', ['scnlog', 'scnlog-copy'])).toBe('scnlog-copy-2')
    expect(copyId('scnlog', ['scnlog', 'scnlog-copy', 'scnlog-copy-2'])).toBe('scnlog-copy-3')
  })

  it('shortens the original rather than the suffix, and never ends on a hyphen', () => {
    const long = `${'a'.repeat(58)}-bcdef`
    const copy = copyId(long, [long])
    expect(copy.length).toBeLessThanOrEqual(64)
    expect(copy.endsWith('-copy')).toBe(true)
    // The cut fell right after the hyphen, which goes with it.
    expect(copy).toBe(`${'a'.repeat(58)}-copy`)
    expect(copy).toMatch(/^[a-z0-9]+(?:-[a-z0-9]+)*$/)
  })

  it('copies the stored body whole and changes only the identifier and the name', () => {
    const draft = filled()
    const body: Record<string, unknown> = { ...toBody(draft), dead: ['old.example.org'], mirrors: true }
    const rule: SiteRule = {
      id: 'my-board',
      name: 'My board',
      group: 'board',
      hosts: ['example.org'],
      version: 1,
      probe: draft.probe,
      mirrors: true,
      steps: 2,
      enabled: true,
      active: true,
      rule: body as never,
      check: null
    }
    const copy = copyBody(rule, [rule], 'copy')
    expect(copy).toEqual({ ...body, id: 'my-board-copy', name: 'My board (copy)' })
    // The original's body is not the copy's: editing one leaves the other.
    expect(body.id).toBe('my-board')
    expect(copy).not.toBe(body)
  })
})
