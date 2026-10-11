import type { LinkFilterRule, LinkFilterRuleRequest } from '@/api/types'
import { MIB } from '@/utils/format'

export type LinkFilterAction = LinkFilterRule['action']
export type LinkFilterNameSyntax = NonNullable<LinkFilterRule['name_syntax']>
type IngressSource = NonNullable<LinkFilterRule['source']>

/** The three things a rule can do with a link it matches (RD-1240-09). */
export const LINK_FILTER_ACTIONS: readonly LinkFilterAction[] = ['hide', 'accept', 'route']

/**
 * The LinkFilter form: sizes in MiB, the pattern and the hoster as typed, empty strings for
 * "no condition". `requestFromForm` turns it into the body the API takes.
 */
export interface LinkFilterForm {
  name: string
  enabled: boolean
  action: LinkFilterAction
  namePattern: string
  nameSyntax: LinkFilterNameSyntax
  sizeMinMib: number | null
  sizeMaxMib: number | null
  extensions: string[]
  hoster: string
  source: IngressSource | null
  packageName: string
  categoryId: string | null
}

export function emptyLinkFilterForm(): LinkFilterForm {
  return {
    name: '',
    enabled: true,
    action: 'hide',
    namePattern: '',
    nameSyntax: 'glob',
    sizeMinMib: null,
    sizeMaxMib: null,
    extensions: [],
    hoster: '',
    source: null,
    packageName: '',
    categoryId: null
  }
}

/** Bytes to MiB, two decimals, as `byteModel` rounds. */
function mib(bytes: number | null | undefined): number | null {
  return typeof bytes === 'number' ? Math.round((bytes / MIB) * 100) / 100 : null
}

/** MiB to bytes; an empty or non-positive field is no bound. */
function bytes(value: number | null | undefined): number | null {
  return typeof value === 'number' && Number.isFinite(value) && value > 0 ? Math.round(value * MIB) : null
}

export function formFromRule(rule: LinkFilterRule): LinkFilterForm {
  return {
    name: rule.name,
    enabled: rule.enabled,
    action: rule.action,
    namePattern: rule.name_pattern ?? '',
    nameSyntax: rule.name_syntax ?? 'glob',
    sizeMinMib: mib(rule.size_min),
    sizeMaxMib: mib(rule.size_max),
    extensions: [...(rule.extensions ?? [])],
    hoster: rule.hoster ?? '',
    source: rule.source ?? null,
    packageName: rule.package_name ?? '',
    categoryId: rule.category_id ?? null
  }
}

/**
 * The body for `POST`/`PUT /api/v1/link-filters`. Only a `route` sends a package and a
 * category — the server drops them for the other two anyway, and sending them would make the
 * form remember something the saved rule does not hold.
 */
export function requestFromForm(form: LinkFilterForm): LinkFilterRuleRequest {
  const route = form.action === 'route'
  return {
    name: form.name.trim(),
    enabled: form.enabled,
    action: form.action,
    name_pattern: form.namePattern.trim() || null,
    name_syntax: form.nameSyntax,
    size_min: bytes(form.sizeMinMib),
    size_max: bytes(form.sizeMaxMib),
    extensions: form.extensions.map(value => value.trim().replace(/^\.+/, '').toLowerCase()).filter(Boolean),
    hoster: form.hoster.trim().toLowerCase() || null,
    source: form.source,
    package_name: route ? form.packageName.trim() || null : null,
    category_id: route ? form.categoryId : null
  }
}

/** The request that makes a copy of `rule` under another name, at the end of the order. */
export function copyRequest(rule: LinkFilterRule, name: string): LinkFilterRuleRequest {
  return { ...requestFromForm(formFromRule(rule)), name }
}

/**
 * The ids in their new order after the rule at `index` moved one step up (`-1`) or down (`1`);
 * `null` when it is already at that end.
 */
export function movedIds(ids: readonly string[], index: number, delta: -1 | 1): string[] | null {
  const target = index + delta
  if (index < 0 || index >= ids.length || target < 0 || target >= ids.length) return null
  const next = [...ids]
  const [moved] = next.splice(index, 1)
  if (moved === undefined) return null
  next.splice(target, 0, moved)
  return next
}

/** The conditions a rule checks, as short tokens; the caller joins and translates the empty case. */
export function conditionTokens(rule: LinkFilterRule, label: {
  source: (source: IngressSource) => string
  size: (bytes: number) => string
}): string[] {
  const parts: string[] = []
  if (rule.source) parts.push(label.source(rule.source))
  if (rule.hoster) parts.push(rule.hoster)
  if (rule.extensions?.length) parts.push(rule.extensions.map(extension => `.${extension}`).join(' '))
  if (rule.name_pattern) parts.push(rule.name_syntax === 'regex' ? `/${rule.name_pattern}/` : rule.name_pattern)
  if (typeof rule.size_min === 'number' && typeof rule.size_max === 'number') {
    parts.push(`${label.size(rule.size_min)}–${label.size(rule.size_max)}`)
  } else if (typeof rule.size_min === 'number') {
    parts.push(`≥ ${label.size(rule.size_min)}`)
  } else if (typeof rule.size_max === 'number') {
    parts.push(`≤ ${label.size(rule.size_max)}`)
  }
  return parts
}
