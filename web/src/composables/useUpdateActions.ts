import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { INSTALL_ENDED, type UpdateOffer } from '@/api/updates'
import { useConfirm } from '@/composables/useConfirm'
import { useUpdateStatus } from '@/composables/useUpdateStatus'
import { translateServerMessage } from '@/i18n/server'

/**
 * "Install and restart" and "Download" for one offered version (RD-180-02), with the rules both
 * places that offer them follow — the details dialog and the notice on the update page
 * (RD-1150-01): the install only behind a confirmation, running downloads asked once more with
 * their count, and the background download of the version offered here with its progress.
 */
export function useUpdateActions(offer: () => UpdateOffer | null) {
  const { t } = useI18n()
  const confirm = useConfirm()
  const { status, installFailure, downloadFailure, followed, install, download } = useUpdateStatus()
  const starting = ref(false)

  /** The install to show: one this page followed, or one for the version offered here. */
  const progress = computed(() => {
    const running = status.value?.install
    if (!running) return null
    return followed.value || running.target_version === offer()?.version ? running : null
  })
  const installing = computed(() => progress.value !== null && !INSTALL_ENDED.includes(progress.value.state))
  const ended = computed(() => progress.value?.state === 'failed' || progress.value?.state === 'rolled_back')
  const refusal = computed(() => installFailure.value ? translateServerMessage(installFailure.value) : null)

  /** The background download of the version offered here. */
  const fetched = computed(() => {
    const background = status.value?.download
    return background && background.version === offer()?.version ? background : null
  })
  const fetching = computed(() => fetched.value?.state === 'downloading')
  const fetchedPercent = computed(() => {
    const background = fetched.value
    if (!background || background.total_bytes <= 0) return 0
    return Math.min(100, Math.round((background.received_bytes / background.total_bytes) * 100))
  })
  const fetchedReason = computed(() => {
    if (downloadFailure.value) return translateServerMessage(downloadFailure.value)
    return fetched.value?.reason ? translateServerMessage({ code: fetched.value.reason }) : null
  })

  /** Confirms and installs; `true` once the service took the install. */
  async function installNow(): Promise<boolean> {
    const offered = offer()
    if (!offered) return false
    const agreed = await confirm({
      title: t('system.updates.install.confirm_title', { version: offered.version }),
      description: t('system.updates.install.confirm_description', { version: offered.version }),
      confirmLabel: t('system.updates.modal.install'),
      confirmIcon: 'i-lucide-refresh-cw'
    })
    if (!agreed) return false
    starting.value = true
    try {
      if (await install()) return true
      // Running downloads are saved by the stop and continue after it; installing anyway is the
      // person's decision, asked once more with the count.
      if (installFailure.value?.code !== 'update.transfers_active') return false
      const anyway = await confirm({
        title: t('system.updates.install.confirm_title', { version: offered.version }),
        description: translateServerMessage(installFailure.value),
        confirmLabel: t('system.updates.install.anyway'),
        confirmIcon: 'i-lucide-refresh-cw'
      })
      return anyway ? await install(true) : false
    } finally {
      starting.value = false
    }
  }

  async function downloadNow(): Promise<void> {
    await download()
  }

  return {
    starting, progress, installing, ended, refusal,
    fetched, fetching, fetchedPercent, fetchedReason, downloadFailure,
    installNow, downloadNow
  }
}
