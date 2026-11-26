/**
 * The settings document over REST (RD-1480-07): read with its entity tag, changed by `PATCH`,
 * which takes only the fields that change.
 *
 * `PUT` replaces the whole document, so a page opened an hour ago wrote its hour-old copy over
 * everything changed since. Nothing here sends it.
 */
import { isRecord } from '@/utils/values'

import { api } from './client'
import type { Settings } from './types'

/** The field names a change holds and their new values; `null` empties a field that can be empty. */
export type SettingsPatch = Record<string, unknown>

/** What a write of the document is answered with; `data` is the whole document as stored. */
export interface SettingsAnswer {
  data?: Settings
  error?: unknown
  /** Absent in the tests' fake client. */
  response?: Response
}

/** The refusal of a write whose `If-Match` names another document than the stored one. */
const DOCUMENT_CHANGED = 'settings.document_changed'

/** Tags of the documents this browser tab stored itself: the last ones, for the events they cause. */
const ownTags: string[] = []
const OWN_TAGS_KEPT = 20

/** The entity tag of an answer, or null where none came along. */
export function documentTag(answer: SettingsAnswer): string | null {
  return answer.response?.headers.get('ETag') ?? null
}

/** Whether the document with this tag is the result of a write this tab sent. */
export function storedHere(tag: string | null): boolean {
  return tag !== null && ownTags.includes(tag)
}

/** Whether a write was refused because the stored document is another one by now. */
export function isDocumentChanged(answer: SettingsAnswer): boolean {
  return isRecord(answer.error) && answer.error.code === DOCUMENT_CHANGED
}

/** The generated schema types a body as the whole document; the service takes any part of it. */
type PatchCall = (
  path: '/api/v1/settings',
  init: { body: SettingsPatch, params?: { header: { 'If-Match': string } } }
) => Promise<SettingsAnswer>

/** Saves `patch`; with `tag`, only while the stored document is still the one it was read from. */
export async function patchSettings(patch: SettingsPatch, tag: string | null = null): Promise<SettingsAnswer> {
  const call = api.PATCH as unknown as PatchCall
  const answer = await call('/api/v1/settings', tag
    ? { body: patch, params: { header: { 'If-Match': tag } } }
    : { body: patch })
  const stored = answer.data ? documentTag(answer) : null
  if (stored) {
    ownTags.push(stored)
    ownTags.splice(0, ownTags.length - OWN_TAGS_KEPT)
  }
  return answer
}
