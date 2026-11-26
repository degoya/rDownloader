/**
 * Three copies of the settings document (RD-1480-07): the stored one as this page last saw it
 * (`base`), the one in the form (`mine`) and the one stored now (`stored`). Field by field, as the
 * service takes them: a nested object or a list is one value.
 */
import { defaultSettings } from '@/settingsDefaults'
import { isPlainRecord } from '@/utils/values'

export type SettingsFields = Record<string, unknown>

/**
 * Fields written by a card with a button of its own, each with its own request: the hot folders'
 * interval, the switched-off plugins, the bandwidth schedule. The form never sends them and takes
 * the stored value as it is, and their change is nobody else's doing that a notice should name.
 */
const SAVED_ELSEWHERE = new Set([
  'hotfolder_poll_seconds', 'disabled_plugins', 'bandwidth_timezone', 'bandwidth_default_profile_id'
])

/** A copy that shares nothing with the form, whose cards change lists and objects in place. */
export function copied<T>(value: T): T {
  return value === undefined ? value : JSON.parse(JSON.stringify(value)) as T
}

/** Keys in order, so an object built here in another order than the service's compares equal. */
function sortedKeys(_key: string, value: unknown): unknown {
  return isPlainRecord(value)
    ? Object.fromEntries(Object.entries(value).sort(([a], [b]) => a < b ? -1 : 1))
    : value
}

/** A field left out of the document is an empty one. */
function same(a: unknown, b: unknown): boolean {
  return JSON.stringify(a ?? null, sortedKeys) === JSON.stringify(b ?? null, sortedKeys)
}

/** The fields the form holds another value in than `base`, the self-saving ones left out. */
export function editedFields(base: SettingsFields, mine: SettingsFields): string[] {
  return [...new Set([...Object.keys(base), ...Object.keys(mine)])]
    .filter(key => !SAVED_ELSEWHERE.has(key) && !same(base[key], mine[key]))
}

/** The part of the document to send: `fields` of `mine` whose value is not the one in `base`. */
export function changedPart(base: SettingsFields, mine: SettingsFields, fields: readonly string[]): SettingsFields {
  return Object.fromEntries(fields
    .filter(key => !same(base[key], mine[key]))
    // Left out of the JSON, an emptied field would keep its value.
    .map(key => [key, copied(mine[key]) ?? null]))
}

export interface MergedSettings {
  /** What the form shows now. */
  merged: SettingsFields
  /** Fields another writer changed, without the self-saving ones. */
  foreign: string[]
  /** Of those, the ones the form holds an unsaved value of its own in; `merged` keeps that one. */
  conflicts: string[]
}

/** Lays the stored document under the form: what is unsaved stays, everything else follows. */
export function mergeStored(base: SettingsFields, mine: SettingsFields, stored: SettingsFields): MergedSettings {
  const result: MergedSettings = { merged: {}, foreign: [], conflicts: [] }
  for (const key of new Set([...Object.keys(base), ...Object.keys(mine), ...Object.keys(stored)])) {
    const edited = !SAVED_ELSEWHERE.has(key) && !same(base[key], mine[key])
    const changed = !SAVED_ELSEWHERE.has(key) && !same(base[key], stored[key])
    const agreed = same(mine[key], stored[key])
    if (changed && !agreed) result.foreign.push(key)
    if (changed && !agreed && edited) result.conflicts.push(key)
    // The form's own value where nothing is to follow, so a list somebody is in is not swapped out.
    result.merged[key] = edited || agreed ? mine[key] : copied(stored[key])
  }
  return result
}

/** The document as it is saved: text trimmed, an empty text as none, host and extension lists tidied. */
export function tidied(document: SettingsFields): SettingsFields {
  const result = { ...document }
  for (const key of ['custom_ca_pem', 'rar_executable', 'passwords_file', 'scripts_directory',
    'excluded_domains_file', 'dlc_service_endpoint', 'vendor_directory', 'managed_tools_manifest_url',
    'upload_remote', 'rclone_executable', 'media_ytdlp_executable', 'media_ffmpeg_executable',
    'gallery_executable', 'record_streamlink_executable']) {
    result[key] = text(result[key]) || null
  }
  result.global_proxy_profile_id = result.global_proxy_profile_id || null
  result.ui_port = result.ui_port || null
  result.record_default_quality = text(result.record_default_quality) || 'best'
  result.cleanup_extensions = unique(list(result.cleanup_extensions)
    .map(extension => extension.trim().replace(/^\./, '').toLowerCase()))
  for (const key of ['gallery_hosts', 'media_hosts']) {
    result[key] = unique(list(result[key])
      .map(host => host.trim().toLowerCase().replace(/^https?:\/\//, '').replace(/^www\./, '').replace(/\/.*$/, '')))
  }
  return result
}

function text(value: unknown): string {
  return typeof value === 'string' ? value.trim() : ''
}

function list(value: unknown): string[] {
  return Array.isArray(value) ? value.filter(entry => typeof entry === 'string') : []
}

function unique(values: string[]): string[] {
  return values.filter((value, index, all) => value && all.indexOf(value) === index)
}

/** The placeholders of a document that has not been read yet, as `base`. */
export function placeholderFields(): SettingsFields {
  return copied(defaultSettings() as SettingsFields)
}
