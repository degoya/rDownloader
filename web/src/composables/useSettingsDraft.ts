import { onUnmounted, reactive, ref, shallowRef, toRaw } from 'vue'

import { documentTag, isDocumentChanged, patchSettings, storedHere, type SettingsAnswer } from '@/api/settingsDocument'
import type { Settings } from '@/api/types'
import { debouncedEventRefresh } from '@/composables/useDebouncedEventRefresh'
import { defaultSettings } from '@/settingsDefaults'
import { useSettingsStore } from '@/stores/settings'
import { MIB } from '@/utils/format'
import { changedPart, copied, editedFields, mergeStored, placeholderFields, tidied, type MergedSettings, type SettingsFields } from '@/utils/settingsMerge'

/** What someone else changed while the page was open, for the notice above the save bar. */
interface ForeignChange {
  /** Fields that hold an unsaved value here: saving overwrites the other one. */
  conflicts: string[]
}

interface SettingsDraftOptions {
  /** Whether the form shows the stored document; until then the page loads it and no event does. */
  loaded: () => boolean
  /** A request of the page itself is on its way: its event is read once it is answered. */
  busy: () => boolean
  /** Every stored document that arrives, whoever asked for it. */
  onStored: (document: Settings) => void
}

/**
 * The settings page's working copy of the document (RD-1480-07): the form, the stored document it
 * started from, and what to send.
 *
 * A save sends the changed fields alone, so a change made elsewhere since — by another tab, the
 * tray, an agent over MCP — is not written over by this page's older copy. `settings.changed` has
 * the stored document read again and laid under the form: what the person has not saved stays
 * theirs, everything else follows, and a field both sides changed is named before a save
 * overwrites the other value. `If-Match` catches the change whose event did not arrive in time.
 */
export function useSettingsDraft(options: SettingsDraftOptions) {
  const { fetchSettings } = useSettingsStore()
  const settings = reactive<Settings>(defaultSettings())
  /** The speed limit as the page edits it, in MiB/s. */
  const speedMiB = ref<number | null>(null)
  const base = shallowRef<SettingsFields>(placeholderFields())
  const tag = ref<string | null>(null)
  const foreign = ref<ForeignChange | null>(null)
  /** Counts the page's own writes, so a read that set out before one is not taken for the newer. */
  let writes = 0

  /** The document in the form. */
  function mine(): SettingsFields {
    const bytes = speedMiB.value ? Math.round(speedMiB.value * MIB) : 0
    const stored = base.value.speed_limit_bytes_per_second
    return {
      ...toRaw(settings) as unknown as SettingsFields,
      // As it is stored while it says the same: a limit of "0" is none.
      speed_limit_bytes_per_second: Number(stored ?? 0) === bytes ? stored : bytes ? String(bytes) : null
    }
  }

  function dirty(): boolean {
    return editedFields(base.value, mine()).length > 0
  }

  /** Lays a stored document under the form, which started from the document `from`. */
  function adopt(stored: Settings, storedTag: string | null, from: SettingsFields): MergedSettings {
    const fields = stored as unknown as SettingsFields
    const result = mergeStored(from, mine(), fields)
    Object.assign(settings, result.merged)
    const limit = result.merged.speed_limit_bytes_per_second
    speedMiB.value = limit ? Number(limit) / MIB : null
    base.value = copied(fields)
    tag.value = storedTag
    options.onStored(stored)
    return result
  }

  function noteForeign({ foreign: changed, conflicts }: MergedSettings): void {
    // A field both sides changed stays named until it is saved or taken over.
    const open = (foreign.value?.conflicts ?? []).filter(key => editedFields(base.value, mine()).includes(key))
    const named = [...new Set([...open, ...conflicts])]
    if (named.length || (changed.length && !storedHere(tag.value))) foreign.value = { conflicts: named }
    else if (foreign.value?.conflicts.length) foreign.value = null
  }

  /** Shows the document as it is stored, unsaved edits given up: the first load, a reset, an import. */
  function replace(answer: SettingsAnswer): void {
    if (!answer.data) return
    writes += 1
    adopt(answer.data, documentTag(answer), mine())
    foreign.value = null
  }

  /** Reads the stored document and lays it under the form. */
  async function refresh(): Promise<void> {
    const seen = writes
    const answer = await fetchSettings()
    // Asked before a write of this page that was answered first: the older of the two documents.
    if (seen !== writes) return stream.schedule()
    if (answer.data && options.loaded()) noteForeign(adopt(answer.data, documentTag(answer), base.value))
  }

  /** Gives up this page's values of the fields both sides changed. */
  function takeForeign(): void {
    for (const key of foreign.value?.conflicts ?? []) {
      const value = copied(base.value[key])
      Object.assign(settings, { [key]: value })
      if (key === 'speed_limit_bytes_per_second') speedMiB.value = value ? Number(value) / MIB : null
    }
    foreign.value = null
  }

  /** Sends what the form holds unsaved, tidied; null when that is nothing. */
  async function send(): Promise<SettingsAnswer | null> {
    const form = copied(mine())
    const edited = editedFields(base.value, form)
    const patch = changedPart(base.value, tidied(form), edited)
    // Edited and still the same once tidied: a blank typed behind a path.
    for (const key of edited) {
      if (!(key in patch)) Object.assign(settings, { [key]: copied(base.value[key]) })
    }
    if (Object.keys(patch).length === 0) return null
    writes += 1
    const answer = await patchSettings(patch, tag.value)
    // What was typed while the request was on its way is an edit against what was sent.
    if (answer.data) adopt(answer.data, documentTag(answer), form)
    return answer
  }

  /**
   * Saves what changed. A document stored elsewhere in between is read and laid under the form;
   * the save goes on where the two touch different fields, and answers `conflict` where they do
   * not, so the person decides.
   */
  async function save(): Promise<{ saved: true } | { conflict: true } | { refused: SettingsAnswer }> {
    let answer = await send()
    if (answer && isDocumentChanged(answer)) {
      await refresh()
      if (foreign.value?.conflicts.length) return { conflict: true }
      answer = await send()
    }
    if (answer && !answer.data) return { refused: answer }
    if (foreign.value?.conflicts.length) foreign.value = null
    return { saved: true }
  }

  const stream = debouncedEventRefresh(['settings.changed'], refresh, { busy: options.busy })
  stream.connect()
  onUnmounted(stream.disconnect)

  return { settings, speedMiB, foreign, dirty, replace, takeForeign, save }
}
