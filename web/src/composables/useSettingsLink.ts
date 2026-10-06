import { computed, toValue, type MaybeRefOrGetter } from 'vue'

import { settingsSearchEntry, settingsSearchLocation } from '@/settingsSearch'
import { SETTINGS_SECTIONS } from '@/settingsSections'
import { revealAnchor } from '@/utils/revealAnchor'

/**
 * A link to a settings card or field by its search anchor (RD-1120-23), never by a page path.
 *
 * The anchor's row in `settingsSearch.ts` says which page and sub-tab hold it; when a card moves
 * to another page, its row moves with it and every link that names the anchor follows, and an
 * anchor id that was retired leads to its successor (`MOVED_SETTINGS_ANCHORS`). The link
 * opens that page and tab, then scrolls to the card or field and outlines it, as the search does:
 * a field takes the focus, a card does not.
 */
export function useSettingsLink(anchor: MaybeRefOrGetter<string>) {
  const entry = computed(() => settingsSearchEntry(toValue(anchor)))

  /** The address of the anchor's page and tab; nothing for an anchor the registry does not know. */
  const to = computed(() => {
    if (!entry.value) return undefined
    const location = settingsSearchLocation(entry.value)
    return location.query ? `${location.path}?tab=${location.query.tab}` : location.path
  })

  /** The i18n key of the page the anchor sits on, for the link's text. */
  const pageLabelKey = computed(() => SETTINGS_SECTIONS.find(section => section.value === entry.value?.section)?.labelKey)

  /** Brings the card or field into view once its page has rendered it. */
  function reveal(): void {
    const target = entry.value
    if (target) void revealAnchor(target.id, { focus: target.kind === 'field' })
  }

  return { entry, to, pageLabelKey, reveal }
}
