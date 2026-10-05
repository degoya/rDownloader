import { computed, ref, type Ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { InstalledPlugin } from '@/api/types'

/**
 * Twenty-four plugins of eight kinds in one flat grid made finding a particular one a scan.
 * The groups are built from the types actually installed, so an installation without, say, a
 * storage plugin is not offered an empty group.
 *
 * This was a `UTabs` bar until RD-107-17, and it stopped being readable through growth rather
 * than through a bug: a tab bar divides one line among its entries, so the eleventh plugin
 * world — `remote-job` — turned ten of the twelve labels into "Benachrich… 3" and "Ordner-Cr… 6".
 * A wrapping chip row grows downward instead, which a settings page has room for; `design.md`
 * carries the rule and the two alternatives that were weighed and rejected.
 */
const ALL_TYPES = '__all__'

/** One installed plugin: the version that is loaded, and the older ones still on disk. */
export interface PluginGroup {
  plugin: InstalledPlugin
  superseded: InstalledPlugin[]
}

/** The installed plugins of the plugins tab, grouped by id and filtered by type. */
export function usePluginGroups(plugins: Ref<InstalledPlugin[]>) {
  const { t } = useI18n()
  const typeTab = ref(ALL_TYPES)

  /**
   * The inventory as plugins rather than as version directories (RD-108-10).
   *
   * Installing never removes the older version — a job already under way keeps the one that
   * started it — so a package of 43 plugins could report 44 entries, and the difference was a
   * leftover version carrying a badge nobody counted. The list is keyed by plugin id now: the
   * loaded version is the card, every superseded version hangs underneath it, and the counts
   * say how many plugins are installed, which is what the number beside "installed" is read as.
   */
  const pluginGroups = computed<PluginGroup[]>(() => {
    const groups = new Map<string, PluginGroup>()
    for (const plugin of plugins.value) {
      const group = groups.get(plugin.id)
      if (!group) groups.set(plugin.id, { plugin, superseded: [] })
      // `active` is the server's answer to which version loads. It comes first in the list, but
      // the grouping does not depend on that: whichever entry claims it becomes the card.
      else if (plugin.active && !group.plugin.active) {
        group.superseded.push(group.plugin)
        group.plugin = plugin
      } else group.superseded.push(plugin)
    }
    return [...groups.values()]
  })
  const installedTypes = computed(() =>
    [...new Set(pluginGroups.value.map(group => group.plugin.plugin_type))].sort((a, b) =>
      t(`plugins.type.${a}`).localeCompare(t(`plugins.type.${b}`))))
  const typeGroups = computed(() => [
    { value: ALL_TYPES, label: t('plugins.type.all'), count: pluginGroups.value.length },
    ...installedTypes.value.map(type => ({
      value: type,
      label: t(`plugins.type.${type}`),
      count: pluginGroups.value.filter(group => group.plugin.plugin_type === type).length
    }))
  ])
  const visibleGroups = computed(() => typeTab.value === ALL_TYPES
    ? pluginGroups.value
    : pluginGroups.value.filter(group => group.plugin.plugin_type === typeTab.value))
  /** Plugin id whose superseded versions are unfolded; one at a time, like the diagnostics. */
  const openSuperseded = ref<string | null>(null)

  function toggleSuperseded(id: string): void {
    openSuperseded.value = openSuperseded.value === id ? null : id
  }

  return { typeTab, pluginGroups, typeGroups, visibleGroups, openSuperseded, toggleSuperseded }
}
