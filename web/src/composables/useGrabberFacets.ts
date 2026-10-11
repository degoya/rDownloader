import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { LinkCandidate } from '@/api/types'
import { useHiddenHosters } from '@/composables/useHiddenHosters'
import { useCollectorStore } from '@/stores/collector'
import { hosterOf } from '@/utils/collectorSort'
import { facetValues, type MirrorFacet } from '@/utils/mirrorGroups'

/**
 * The LinkGrabber's filters: the three mirror facets, the state filter, the hidden hosters and
 * the links LinkFilter rules hid.
 *
 * `'all'` = facet off; an empty string is not a legal select value (Reka UI throws on it).
 *
 * The three facets are one control with two effects (RD-110-19, `design.md`): inside a mirror
 * group the server uses them to choose the member the queue will fetch, and outside one they
 * hide what cannot satisfy them. They are the standing preference, so they are written to the
 * server rather than kept here — the computed refs only mirror the last answer it gave.
 */
export function useGrabberFacets() {
  const { t } = useI18n()
  const collector = useCollectorStore()

  const hosterFilter = computed({
    get: () => collector.mirrorPreference.hoster ?? 'all',
    set: (value: string) => void applyFacet('hoster', value)
  })
  const qualityFilter = computed({
    get: () => collector.mirrorPreference.quality ?? 'all',
    set: (value: string) => void applyFacet('quality', value)
  })
  const languageFilter = computed({
    get: () => collector.mirrorPreference.language ?? 'all',
    set: (value: string) => void applyFacet('language', value)
  })
  /** True while a facet change is in flight; the selects stay readable but refuse a second one. */
  const facetBusy = ref(false)

  async function applyFacet(facet: MirrorFacet, value: string): Promise<void> {
    facetBusy.value = true
    await collector.setMirrorPreference({
      ...collector.mirrorPreference,
      [facet]: value === 'all' ? null : value
    })
    facetBusy.value = false
  }

  const stateFilter = ref<LinkCandidate['state'] | 'all'>('all')

  /** Hosters hidden from the list, several at once (RD-130-21); stored with the facets. */
  const hiddenHosters = useHiddenHosters()

  /**
   * Links a LinkFilter rule hid (RD-1240-09) are drawn only while this is on. View state, like
   * the state filter: the rule is what stands, and the switch is a look at what it kept back.
   */
  const showFiltered = ref(false)
  const filteredCount = computed(() => collector.candidates.filter(candidate => candidate.hidden_by_filter).length)

  /** The facets and the state filter, which "clear filters" resets; hidden hosters have their own way back. */
  const facetFilterActive = computed(() => hosterFilter.value !== 'all' || stateFilter.value !== 'all'
    || qualityFilter.value !== 'all' || languageFilter.value !== 'all')
  /**
   * Whether the list shows less than the LinkGrabber holds. Hidden hosters count only while they
   * hide a link: a hoster hidden last week with nothing in the list now must not turn every
   * enqueue into a partial one or refuse a reorder of a list that is in fact whole.
   */
  const filterActive = computed(() => facetFilterActive.value || hiddenHosters.hiddenLinks.value.length > 0
    || (!showFiltered.value && filteredCount.value > 0))
  /** Hoster options come from the unfiltered list so the active choice never disappears. */
  const hosterItems = computed(() => {
    const values = new Set([...collector.candidates.map(hosterOf)].filter(Boolean))
    if (collector.mirrorPreference.hoster) values.add(collector.mirrorPreference.hoster)
    return [
      { label: t('linkgrabber.filter.all_hosters'), value: 'all' },
      ...[...values].sort().map(hoster => ({ label: hoster, value: hoster }))
    ]
  })
  /**
   * The values each facet actually takes in this list, so the select offers no dead option. The
   * value in force is added back even when nothing carries it any more: a preference that
   * vanished from its own control could not be cleared.
   */
  function facetItems(facet: 'quality' | 'language', allLabel: string) {
    const values = facetValues(collector.candidates, facet)
    const current = collector.mirrorPreference[facet]
    if (current && !values.includes(current)) values.push(current)
    return [{ label: allLabel, value: 'all' }, ...values.sort().map(value => ({ label: value, value }))]
  }
  const qualityItems = computed(() => facetItems('quality', t('linkgrabber.filter.all_qualities')))
  const languageItems = computed(() => facetItems('language', t('linkgrabber.filter.all_languages')))
  const stateItems = computed(() => [
    { label: t('linkgrabber.filter.all_states'), value: 'all' },
    ...(['online', 'offline', 'duplicate', 'checking', 'resolving', 'unsupported', 'error'] as const)
      .map(state => ({ label: t(`linkgrabber.candidate.state.${state}`), value: state }))
  ])

  /** Clears every facet in one request, plus the state filter, which is view state only. */
  async function clearFilters(): Promise<void> {
    stateFilter.value = 'all'
    if (qualityFilter.value === 'all' && languageFilter.value === 'all' && hosterFilter.value === 'all') return
    facetBusy.value = true
    await collector.setMirrorPreference({ quality: null, language: null, hoster: null, hidden_hosters: collector.mirrorPreference.hidden_hosters })
    facetBusy.value = false
  }

  return {
    hosterFilter, qualityFilter, languageFilter, stateFilter, facetBusy, hiddenHosters,
    showFiltered, filteredCount, facetFilterActive, filterActive, hosterItems, qualityItems, languageItems, stateItems, clearFilters
  }
}
