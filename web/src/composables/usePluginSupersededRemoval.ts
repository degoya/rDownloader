import { useToast } from '@nuxt/ui/composables'
import type { Ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { SupersededRemoval } from '@/api/types'
import { useConfirm } from '@/composables/useConfirm'
import { translateServerMessage } from '@/i18n/server'

/** Whose superseded versions go: every plugin's, or one plugin's, by its card. */
interface SupersededScope {
  /** How many the list shows, for the confirmation. */
  count: number
  plugin?: { id: string, name: string }
}

/**
 * Removing every superseded version at once (RD-1140-04): of every plugin from the plugins tab,
 * of one plugin from its card. Removing them one confirmation at a time was the chore the owner
 * asked to end.
 *
 * One confirmation with the number, then a toast with what went and what stayed. The service
 * keeps the version that runs, the one the next start loads, the one under test and every version
 * unfinished work is bound to, and names each with its reason; the toast lists those, so a
 * version that is still on the card afterwards says why. A refused request lands in the tab's
 * own `error`, like the single removal's.
 */
export function usePluginSupersededRemoval(options: {
  message: Ref<string | null>
  error: Ref<string | null>
  /** Re-reads the inventory the removal changed. */
  done: () => Promise<unknown>
}) {
  const { t } = useI18n()
  const toast = useToast()
  const confirm = useConfirm()

  async function removeSuperseded(scope: SupersededScope): Promise<void> {
    const { count, plugin } = scope
    const confirmed = await confirm({
      title: t('plugins.remove.superseded_all_title'),
      description: plugin
        ? t('plugins.remove.superseded_plugin_description', { count, name: plugin.name }, count)
        : t('plugins.remove.superseded_all_description', { count }, count),
      confirmLabel: t('common.actions.delete'),
      confirmIcon: 'i-lucide-trash-2',
      destructive: true
    })
    if (!confirmed) return
    options.message.value = null
    options.error.value = null
    const response = plugin
      ? await api.DELETE('/api/v1/plugins/{id}/superseded', { params: { path: { id: plugin.id } } })
      : await api.DELETE('/api/v1/plugins/superseded')
    if (response.data) announce(response.data)
    else options.error.value = responseError(response)
    await options.done()
  }

  function announce(result: SupersededRemoval): void {
    const kept = result.kept.map(entry =>
      `${entry.name} v${entry.version}: ${translateServerMessage(entry.reason)}`)
    toast.add({
      title: t('plugins.remove.superseded_done', { removed: result.removed.length, kept: kept.length }),
      ...(kept.length ? { description: kept.join(' · ') } : {}),
      color: kept.length ? 'warning' : 'success',
      icon: kept.length ? 'i-lucide-circle-alert' : 'i-lucide-trash-2'
    })
  }

  return { removeSuperseded }
}
