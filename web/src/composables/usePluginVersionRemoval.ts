import type { Ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api } from '@/api/client'
import type { InstalledPlugin } from '@/api/types'
import { displayName } from '@/components/settings/pluginDisplay'
import { useConfirm } from '@/composables/useConfirm'
import { serverMessageFrom, translateServerMessage } from '@/i18n/server'

/** The plugins tab's removal of one installed version, and how a removal's answer is shown. */
export function usePluginVersionRemoval(card: {
  message: Ref<string | null>
  error: Ref<string | null>
  /** Reads the inventory again once a removal has been answered. */
  done: () => Promise<unknown>
}) {
  const { t } = useI18n()
  const confirm = useConfirm()
  const { message, error } = card

  /** Removes a package this build refuses, once the user asks for it. */
  async function removeVersion(plugin: { id: string, version: string }): Promise<void> {
    error.value = null
    message.value = null
    const response = await api.DELETE('/api/v1/plugins/{id}/{version}', {
      params: { path: { id: plugin.id, version: plugin.version } }
    })
    showAnswer(response)
    await card.done()
  }

  /**
   * The coded message of a removal's answer, in the success or the error line. Through the client
   * like every request (WEB-03): a raw `fetch` here noticed neither a lapsed session nor a dropped
   * connection.
   */
  function showAnswer(response: { data?: unknown, error?: unknown }): void {
    const serverMessage = serverMessageFrom(response.data ?? response.error)
    const text = serverMessage ? translateServerMessage(serverMessage) : null
    if (response.data) message.value = text
    else error.value = text
  }

  /** Asks first: removing a plugin deletes it from disk, and re-adding means re-installing it. */
  async function confirmRemove(plugin: InstalledPlugin): Promise<void> {
    const confirmed = await confirm({
      title: t('plugins.remove.title'),
      description: t('plugins.remove.description', { name: displayName(plugin) }),
      confirmLabel: t('common.actions.delete'),
      confirmIcon: 'i-lucide-trash-2',
      destructive: true
    })
    if (confirmed) await removeVersion(plugin)
  }

  /**
   * Removes one superseded version, once the user asks for it.
   *
   * Also destructive, and asked for separately: the version that is loaded stays, so the
   * sentence has to name which of the two is going. The service refuses with
   * `plugin.version_in_use` when a job is still bound to it, and that answer is what the alert
   * above the list then shows.
   */
  async function confirmRemoveSuperseded(active: InstalledPlugin, old: InstalledPlugin): Promise<void> {
    const confirmed = await confirm({
      title: t('plugins.remove.superseded_title'),
      description: t('plugins.remove.superseded_description', {
        name: displayName(active),
        version: old.version,
        active: active.version
      }),
      confirmLabel: t('common.actions.delete'),
      confirmIcon: 'i-lucide-trash-2',
      destructive: true
    })
    if (confirmed) await removeVersion(old)
  }

  return { showAnswer, removeVersion, confirmRemove, confirmRemoveSuperseded }
}
