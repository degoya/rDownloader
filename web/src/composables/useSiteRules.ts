import { computed, ref, type Ref } from 'vue'

import { api, responseError } from '@/api/client'
import type {
  SiteRule,
  SiteRuleGroup,
  SiteRuleImportResult,
  SiteRuleTestResult
} from '@/api/types'
import { useFetchState } from '@/composables/useFetchState'
import { duplicateName } from '@/utils/copyName'

/**
 * The site rules of this installation, and the writes the settings page performs (RD-110-08).
 *
 * The rule *body* stays the shape the service speaks — the seven step kinds live in
 * `rd-siterules` and a second TypeScript copy of them would drift the first time a step grows
 * a field. What this module owns instead is the translation between that body and the form
 * the editor draws: `RuleDraft` is one flat object of named fields, and `toBody`/`fromRule`
 * are the only two places that know how it maps onto the wire.
 */

/** What `rd_siterules::Rule::validate` accepts for an identifier and a name. */
const MAX_RULE_ID_LENGTH = 64
const MAX_RULE_NAME_LENGTH = 120

/** The seven step kinds, exactly as the rule format spells them. */
export const STEP_KINDS = ['fetch', 'fetch-json', 'regex', 'decode', 'form', 'redirect', 'captcha'] as const
export type StepKind = typeof STEP_KINDS[number]

/** What `decode` understands. */
export const ENCODINGS = ['base64', 'hex', 'rot13', 'url', 'js-string'] as const
export type Encoding = typeof ENCODINGS[number]

export const PACKAGE_SOURCES = ['title', 'regex', 'variable'] as const
export type PackageSource = typeof PACKAGE_SOURCES[number]

/** One step as the form holds it: every field of every kind, only some of them shown. */
export interface StepDraft {
  kind: StepKind
  url: string
  into: string
  from: string
  path: string
  pattern: string
  all: boolean
  encoding: Encoding
  /** `name=value` per line. */
  fields: string
  challenge: string
  sitekey: string
}

/** One rule as the form holds it. Lists are newline-separated text, because that is what a
 *  textarea is; nothing else in this file treats them as strings. */
export interface RuleDraft {
  id: string
  name: string
  group: string
  version: number
  hosts: string
  paths: string
  dead: string
  probe: string
  checked: string
  mirrors: boolean
  packageFrom: PackageSource
  packagePattern: string
  packageSource: string
  packageName: string
  steps: StepDraft[]
  enabled: boolean
}

export function emptyStep(kind: StepKind = 'fetch'): StepDraft {
  return {
    kind,
    url: '',
    into: '',
    from: '',
    path: '',
    pattern: '',
    all: kind === 'regex',
    encoding: 'base64',
    fields: '',
    challenge: '',
    sitekey: ''
  }
}

export function emptyDraft(): RuleDraft {
  return {
    id: '',
    name: '',
    group: 'board',
    version: 1,
    hosts: '',
    paths: '',
    dead: '',
    probe: '',
    checked: new Date().toISOString().slice(0, 10),
    mirrors: false,
    packageFrom: 'title',
    packagePattern: '',
    packageSource: '',
    packageName: '',
    steps: [emptyStep('fetch'), emptyStep('regex')],
    enabled: false
  }
}

function lines(value: string): string[] {
  return value.split('\n').map(line => line.trim()).filter(line => line.length > 0)
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function text(value: unknown): string {
  return typeof value === 'string' ? value : ''
}

/** The form fields of a `form` step, as the wire wants them: an object, not lines. */
function fieldMap(value: string): Record<string, string> {
  const map: Record<string, string> = {}
  for (const line of lines(value)) {
    const split = line.indexOf('=')
    if (split <= 0) continue
    map[line.slice(0, split).trim()] = line.slice(split + 1).trim()
  }
  return map
}

function stepBody(step: StepDraft): Record<string, unknown> {
  const optional = (key: string, value: string): Record<string, string> =>
    value.trim() ? { [key]: value.trim() } : {}
  switch (step.kind) {
    case 'fetch':
      return { kind: 'fetch', ...optional('url', step.url), ...optional('into', step.into) }
    case 'fetch-json':
      return { kind: 'fetch-json', url: step.url.trim(), path: step.path.trim(), into: step.into.trim() }
    case 'regex':
      return {
        kind: 'regex',
        pattern: step.pattern,
        ...optional('from', step.from),
        into: step.into.trim(),
        ...(step.all ? { all: true } : {})
      }
    case 'decode':
      return { kind: 'decode', encoding: step.encoding, from: step.from.trim(), into: step.into.trim() }
    case 'form':
      return {
        kind: 'form',
        url: step.url.trim(),
        fields: fieldMap(step.fields),
        ...optional('into', step.into)
      }
    case 'redirect':
      return { kind: 'redirect', from: step.from.trim(), into: step.into.trim() }
    case 'captcha':
      return {
        kind: 'captcha',
        challenge: step.challenge.trim(),
        ...optional('sitekey', step.sitekey),
        ...optional('into', step.into)
      }
  }
}

function stepDraft(value: unknown): StepDraft {
  if (!isRecord(value)) return emptyStep()
  const kind = STEP_KINDS.find(candidate => candidate === value.kind) ?? 'fetch'
  const fields = isRecord(value.fields)
    ? Object.entries(value.fields).map(([name, entry]) => `${name}=${String(entry)}`).join('\n')
    : ''
  const encoding = ENCODINGS.find(candidate => candidate === value.encoding) ?? 'base64'
  return {
    kind,
    url: text(value.url),
    into: text(value.into),
    from: text(value.from),
    path: text(value.path),
    pattern: text(value.pattern),
    all: value.all === true,
    encoding,
    fields,
    challenge: text(value.challenge),
    sitekey: text(value.sitekey)
  }
}

/** The rule body this draft describes, ready for the service to parse and validate. */
export function toBody(draft: RuleDraft): Record<string, unknown> {
  const paths = lines(draft.paths)
  const dead = lines(draft.dead)
  const packageSource =
    draft.packageFrom === 'title'
      ? { from: 'title' }
      : draft.packageFrom === 'variable'
        ? { from: 'variable', name: draft.packageName.trim() }
        : {
            from: 'regex',
            pattern: draft.packagePattern,
            ...(draft.packageSource.trim() ? { source: draft.packageSource.trim() } : {})
          }
  return {
    id: draft.id.trim(),
    name: draft.name.trim(),
    group: draft.group.trim(),
    version: draft.version,
    match: { hosts: lines(draft.hosts), ...(paths.length ? { paths } : {}) },
    ...(dead.length ? { dead } : {}),
    steps: draft.steps.map(stepBody),
    package: packageSource,
    ...(draft.mirrors ? { mirrors: true } : {}),
    probe: draft.probe.trim(),
    checked: draft.checked
  }
}

/** The draft behind one rule of the list, so pressing edit fills the form. */
export function fromRule(rule: SiteRule): RuleDraft {
  const body = isRecord(rule.rule) ? rule.rule : {}
  const match = isRecord(body.match) ? body.match : {}
  const pack = isRecord(body.package) ? body.package : { from: 'title' }
  const packageFrom = PACKAGE_SOURCES.find(candidate => candidate === pack.from) ?? 'title'
  const steps = Array.isArray(body.steps) ? body.steps.map(stepDraft) : [emptyStep()]
  return {
    id: rule.id,
    name: rule.name,
    group: rule.group,
    version: rule.version || 1,
    hosts: (Array.isArray(match.hosts) ? match.hosts : []).join('\n'),
    paths: (Array.isArray(match.paths) ? match.paths : []).join('\n'),
    dead: (Array.isArray(body.dead) ? body.dead : []).join('\n'),
    probe: rule.probe,
    checked: text(body.checked) || new Date().toISOString().slice(0, 10),
    mirrors: rule.mirrors,
    packageFrom,
    packagePattern: text(pack.pattern),
    packageSource: text(pack.source),
    packageName: text(pack.name),
    steps: steps.length ? steps : [emptyStep()],
    enabled: rule.enabled
  }
}

/**
 * The identifier of a copy (RD-130-07): `<id>-copy`, then `<id>-copy-2` and on, the first one
 * no rule carries. The identifier is lowercase kebab-case of at most 64 characters, so the
 * original is shortened before the suffix rather than the suffix cut off, and a hyphen the cut
 * leaves at its end goes too.
 */
export function copyId(original: string, existingIds: Iterable<string>): string {
  const used = new Set(existingIds)
  for (let index = 1; index <= used.size + 1; index += 1) {
    const suffix = index === 1 ? '-copy' : `-copy-${index}`
    const base = original.slice(0, MAX_RULE_ID_LENGTH - suffix.length).replace(/-+$/, '')
    const candidate = `${base}${suffix}`
    if (!used.has(candidate)) return candidate
  }
  // The loop must find a free identifier because it checks one more candidate than there are.
  return original
}

/**
 * The body of a copy of `rule`: the stored body unchanged except for a free identifier and a
 * copy name, so the copy recognises exactly what the original does until somebody edits it.
 * Copied from the body rather than through the editor's draft, which would drop a field the
 * form does not draw.
 */
export function copyBody(rule: SiteRule, existing: SiteRule[], copyLabel: string): Record<string, unknown> {
  const body = isRecord(rule.rule) ? rule.rule : {}
  return {
    ...body,
    id: copyId(rule.id, existing.map(entry => entry.id)),
    name: duplicateName(rule.name, existing.map(entry => entry.name), copyLabel, MAX_RULE_NAME_LENGTH)
  }
}

/** Whether the draft carries what the service demands before it will even look at it. */
export function draftComplete(draft: RuleDraft): boolean {
  return draft.id.trim().length > 0
    && draft.name.trim().length > 0
    && draft.group.trim().length > 0
    && lines(draft.hosts).length > 0
    && draft.steps.length > 0
    && draft.probe.trim().length > 0
}

export interface SiteRulesApi {
  rules: Ref<SiteRule[]>
  groups: Ref<SiteRuleGroup[]>
  loading: Ref<boolean>
  loadError: Ref<string | null>
  pending: Ref<boolean>
  busyId: Ref<string | null>
  error: Ref<string | null>
  /** Groups in order, each with the rules that carry it. */
  byGroup: Ref<{ group: SiteRuleGroup, rules: SiteRule[] }[]>
  refresh: () => Promise<void>
  setRuleEnabled: (rule: SiteRule, enabled: boolean) => Promise<boolean>
  setGroupEnabled: (group: string, enabled: boolean) => Promise<boolean>
  save: (draft: RuleDraft, editingId: string | null) => Promise<boolean>
  remove: (id: string) => Promise<boolean>
  /** Creates a switched-off copy of `rule` and answers with the copy's identifier. */
  duplicate: (rule: SiteRule, copyLabel: string) => Promise<string | null>
  test: (draft: RuleDraft, address: string) => Promise<SiteRuleTestResult | null>
  exportRules: () => Promise<unknown | null>
  /** Sends a file's text exactly as it was read: the signed release file's signature covers
   *  those bytes, and a parsed and re-serialised copy would no longer be what was signed. */
  importRules: (text: string) => Promise<SiteRuleImportResult | null>
}

export function useSiteRules(): SiteRulesApi {
  const rules = ref<SiteRule[]>([])
  const groups = ref<SiteRuleGroup[]>([])
  const { loading, loadError, load } = useFetchState()
  const pending = ref(false)
  const busyId = ref<string | null>(null)
  const error = ref<string | null>(null)

  const byGroup = computed(() =>
    groups.value.map(group => ({
      group,
      rules: rules.value.filter(rule => rule.group === group.group)
    }))
  )

  async function refresh(): Promise<void> {
    await load(async () => {
      const response = await api.GET('/api/v1/site-rules')
      if (!response.data) return responseError(response)
      rules.value = response.data.rules
      groups.value = response.data.groups
      return null
    })
  }

  /** Runs one write, keeps the list in step, and reports the failure in the reader's language. */
  async function write(id: string | null, run: () => Promise<{ error?: unknown }>): Promise<boolean> {
    busyId.value = id
    pending.value = true
    error.value = null
    const response = await run()
    pending.value = false
    busyId.value = null
    if (response.error !== undefined) {
      error.value = responseError(response)
      return false
    }
    await refresh()
    return true
  }

  return {
    rules,
    groups,
    loading,
    loadError,
    pending,
    busyId,
    error,
    byGroup,
    refresh,
    setRuleEnabled: (rule, enabled) =>
      write(rule.id, () =>
        api.PUT('/api/v1/site-rules/{id}/enabled', {
          params: { path: { id: rule.id } },
          body: { enabled }
        })
      ),
    setGroupEnabled: (group, enabled) =>
      write(`group:${group}`, () =>
        api.PUT('/api/v1/site-rule-groups/{group}/enabled', {
          params: { path: { group } },
          body: { enabled }
        })
      ),
    save: (draft, editingId) =>
      write(editingId, () => {
        const body = { rule: toBody(draft) as never, enabled: draft.enabled }
        return editingId
          ? api.PUT('/api/v1/site-rules/{id}', { params: { path: { id: editingId } }, body })
          : api.POST('/api/v1/site-rules', { body })
      }),
    remove: id => write(id, () => api.DELETE('/api/v1/site-rules/{id}', { params: { path: { id } } })),
    async duplicate(rule, copyLabel) {
      const body = copyBody(rule, rules.value, copyLabel)
      // Switched off, like every rule that did not come out of the editor: two rules claiming
      // the same hosts would otherwise both be consulted the moment the copy exists.
      const stored = await write(`copy:${rule.id}`, () =>
        api.POST('/api/v1/site-rules', { body: { rule: body as never, enabled: false } })
      )
      return stored ? String(body.id) : null
    },
    async test(draft, address) {
      pending.value = true
      error.value = null
      const response = await api.POST('/api/v1/site-rules/test', {
        body: { rule: toBody(draft) as never, address }
      })
      pending.value = false
      if (!response.data) {
        error.value = responseError(response)
        return null
      }
      return response.data
    },
    async exportRules() {
      error.value = null
      const response = await api.GET('/api/v1/site-rules/export')
      if (!response.data) {
        error.value = responseError(response)
        return null
      }
      return response.data
    },
    async importRules(text) {
      pending.value = true
      error.value = null
      const response = await api.POST('/api/v1/site-rules/import', {
        body: text as never,
        bodySerializer: () => text
      })
      pending.value = false
      if (!response.data) {
        error.value = responseError(response)
        return null
      }
      await refresh()
      return response.data
    }
  }
}
