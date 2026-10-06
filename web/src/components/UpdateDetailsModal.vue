<script setup lang="ts">
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { hintKey, INSTALL_ENDED, type InstallKind, type UpdateOffer } from '@/api/updates'
import CopyField from '@/components/CopyField.vue'
import { useConfirm } from '@/composables/useConfirm'
import { useUpdateStatus } from '@/composables/useUpdateStatus'
import { translateServerMessage } from '@/i18n/server'
import { formatBytes, formatLongMoment } from '@/utils/format'

/**
 * A newer version, its notes and what to do about it (RD-180-01): the download for a build this
 * cannot update, the package manager's command for everything a package manager owns, and for a
 * portable archive or the Windows installer "Install and restart" (RD-180-02), followed here
 * through the restart to the new version or back to the old one, whose reason is shown with a
 * way to try again. There "Download" fetches and verifies the version in the background with its
 * progress, and the install then uses that file; the browser's download stays a small link
 * (owner, 2026-10-01). The notes are plain text from the signed manifest, rendered as text.
 */
const props = defineProps<{ offer: UpdateOffer | null, kind: InstallKind }>()
const open = defineModel<boolean>('open', { required: true })
const { t } = useI18n()
const confirm = useConfirm()
const { status, installFailure, downloadFailure, reconnecting, lost, followed, install, download } = useUpdateStatus()
const starting = ref(false)

const kindLabel = computed(() => t(`system.updates.kind.${props.kind}`))

/** The install to show: one this page followed, or one for the version offered here. */
const progress = computed(() => {
  const install = status.value?.install
  if (!install) return null
  return followed.value || install.target_version === props.offer?.version ? install : null
})
const installing = computed(() => progress.value !== null && !INSTALL_ENDED.includes(progress.value.state))
const progressText = computed(() => {
  const install = progress.value
  if (!install) return ''
  return t(`system.updates.install.state.${install.state}`, {
    version: install.target_version,
    from: install.from_version
  })
})
const progressReason = computed(() =>
  progress.value?.reason ? translateServerMessage({ code: progress.value.reason }) : null)
const refusal = computed(() => installFailure.value ? translateServerMessage(installFailure.value) : null)
const ended = computed(() => progress.value?.state === 'failed' || progress.value?.state === 'rolled_back')

/** The background download of the version offered here. */
const fetched = computed(() => {
  const download = status.value?.download
  return download && download.version === props.offer?.version ? download : null
})
const fetching = computed(() => fetched.value?.state === 'downloading')
const fetchedPercent = computed(() => {
  const download = fetched.value
  if (!download || download.total_bytes <= 0) return 0
  return Math.min(100, Math.round((download.received_bytes / download.total_bytes) * 100))
})
const fetchedReason = computed(() => {
  if (downloadFailure.value) return translateServerMessage(downloadFailure.value)
  return fetched.value?.reason ? translateServerMessage({ code: fetched.value.reason }) : null
})

async function installNow(): Promise<void> {
  const offer = props.offer
  if (!offer) return
  const agreed = await confirm({
    title: t('system.updates.install.confirm_title', { version: offer.version }),
    description: t('system.updates.install.confirm_description', { version: offer.version }),
    confirmLabel: t('system.updates.modal.install'),
    confirmIcon: 'i-lucide-refresh-cw'
  })
  if (!agreed) return
  starting.value = true
  try {
    if (await install()) return
    // Running downloads are saved by the stop and continue after it; installing anyway is the
    // person's decision, asked once more with the count.
    if (installFailure.value?.code !== 'update.transfers_active') return
    const anyway = await confirm({
      title: t('system.updates.install.confirm_title', { version: offer.version }),
      description: translateServerMessage(installFailure.value),
      confirmLabel: t('system.updates.install.anyway'),
      confirmIcon: 'i-lucide-refresh-cw'
    })
    if (anyway) await install(true)
  } finally {
    starting.value = false
  }
}

async function downloadNow(): Promise<void> {
  await download()
}

function reload(): void {
  window.location.reload()
}
</script>

<template>
  <UModal
    v-model:open="open"
    :title="offer ? t('system.updates.modal.title', { version: offer.version }) : t('system.updates.title')"
    :description="offer ? t('system.updates.modal.released', { when: formatLongMoment(offer.released_at) }) : undefined"
  >
    <template #body>
      <div class="space-y-5" data-testid="update-details">
        <section v-if="progress" data-testid="update-install-progress" aria-live="polite">
          <UAlert
            v-if="ended"
            color="error"
            icon="i-lucide-undo-2"
            :title="progressText"
            :description="progressReason ?? undefined"
            data-testid="update-install-outcome"
          />
          <UAlert
            v-else-if="progress.state === 'done'"
            color="success"
            icon="i-lucide-circle-check"
            :title="progressText"
            :description="t('system.updates.install.reload_hint')"
            data-testid="update-install-outcome"
          />
          <UAlert
            v-else-if="lost"
            color="error"
            icon="i-lucide-circle-alert"
            :title="progressText"
            :description="t('system.updates.install.lost')"
            data-testid="update-install-lost"
          />
          <div v-else class="flex items-start gap-2 text-sm text-toned">
            <UIcon name="i-lucide-loader-circle" class="mt-0.5 size-4 shrink-0 animate-spin" />
            <div>
              <p>{{ progressText }}</p>
              <p v-if="progress.state === 'downloading' && fetching" class="mt-1 text-muted">
                {{ t('system.updates.download.progress', { received: formatBytes(String(fetched?.received_bytes ?? 0)), total: formatBytes(String(fetched?.total_bytes ?? 0)) }) }}
              </p>
              <p v-if="reconnecting" class="mt-1 text-muted" data-testid="update-install-reconnecting">{{ t('system.updates.install.reconnecting') }}</p>
            </div>
          </div>
        </section>
        <template v-if="offer">
          <section>
            <p class="eyebrow">{{ t('system.updates.modal.notes') }}</p>
            <p v-if="offer.notes" class="mt-2 max-h-72 overflow-y-auto whitespace-pre-line text-sm leading-6 text-toned" data-testid="update-notes">{{ offer.notes }}</p>
            <p v-else class="mt-2 text-sm text-muted">{{ t('system.updates.modal.no_notes') }}</p>
            <ULink :to="offer.release_url" target="_blank" rel="noopener" class="mt-2 inline-flex items-center gap-1 text-sm text-primary">
              {{ t('system.updates.modal.full_notes') }}
              <UIcon name="i-lucide-external-link" class="size-3.5" />
            </ULink>
          </section>
          <section v-if="offer.action === 'command' && offer.command" data-testid="update-command">
            <p class="text-sm text-toned">{{ t('system.updates.modal.command_hint', { kind: kindLabel }) }}</p>
            <CopyField class="mt-2" :value="offer.command" :label="t('system.updates.modal.copy')" icon-only />
            <p v-if="offer.hint" class="mt-2 text-xs text-muted">{{ t(hintKey(offer.hint)) }}</p>
            <ULink
              v-if="offer.download_url && (kind === 'deb' || kind === 'rpm')"
              :to="offer.download_url"
              target="_blank"
              rel="noopener"
              class="mt-2 inline-flex items-center gap-1 text-sm text-primary"
              data-testid="update-package-download"
            >
              {{ t('system.updates.modal.download_package') }}
              <UIcon name="i-lucide-download" class="size-3.5" />
            </ULink>
          </section>
          <section v-else-if="offer.action === 'install'" data-testid="update-install">
            <p class="text-sm text-toned">{{ t('system.updates.modal.install_hint') }}</p>
            <p
              class="mt-2 text-xs"
              :class="offer.rollback_available === false ? 'text-warning' : 'text-muted'"
              data-testid="update-install-rollback"
            >
              {{ offer.rollback_available === false ? t('system.updates.modal.rollback_unavailable') : t('system.updates.modal.rollback_automatic') }}
            </p>
            <div v-if="fetched && !installing" class="mt-3 text-sm" data-testid="update-download-progress" aria-live="polite">
              <template v-if="fetched.state === 'downloading'">
                <p class="text-toned">{{ t('system.updates.download.downloading', { version: fetched.version }) }}</p>
                <div class="mt-2 flex items-center gap-2">
                  <UProgress :model-value="fetchedPercent" size="xs" class="flex-1" />
                  <span class="shrink-0 text-xs text-muted tabular-nums">
                    {{ t('system.updates.download.progress', { received: formatBytes(String(fetched.received_bytes)), total: formatBytes(String(fetched.total_bytes)) }) }}
                  </span>
                </div>
              </template>
              <p v-else-if="fetched.state === 'ready'" class="flex items-start gap-1.5 text-success" data-testid="update-download-ready">
                <UIcon name="i-lucide-circle-check" class="mt-0.5 size-4 shrink-0" />
                {{ t('system.updates.download.ready', { version: fetched.version }) }}
              </p>
              <p v-else class="text-error" data-testid="update-download-failed">
                {{ t('system.updates.download.failed', { version: fetched.version }) }}
                <template v-if="fetchedReason"> {{ fetchedReason }}</template>
              </p>
            </div>
            <p v-else-if="downloadFailure" class="mt-2 text-sm text-error" data-testid="update-download-failed">{{ fetchedReason }}</p>
            <p v-if="refusal" class="mt-2 text-sm text-error" data-testid="update-install-refused">{{ refusal }}</p>
            <ULink
              v-if="offer.download_url"
              :to="offer.download_url"
              target="_blank"
              rel="noopener"
              class="mt-3 inline-flex items-center gap-1 text-xs text-muted"
              data-testid="update-download-manual"
            >
              {{ t('system.updates.modal.download_manual') }}
              <UIcon name="i-lucide-external-link" class="size-3" />
            </ULink>
          </section>
          <section v-else data-testid="update-download">
            <p class="text-sm text-toned">{{ t('system.updates.modal.download_hint') }}</p>
            <p v-if="offer.download_sha256" class="mt-2 break-all font-mono text-xs text-muted">
              SHA-256 {{ offer.download_sha256 }}<template v-if="offer.download_size"> · {{ formatBytes(String(offer.download_size)) }}</template>
            </p>
          </section>
        </template>
      </div>
    </template>
    <template #footer>
      <UButton color="neutral" variant="outline" :label="t('system.updates.modal.close')" @click="open = false" />
      <UButton
        v-if="progress?.state === 'done'"
        icon="i-lucide-rotate-cw"
        :label="t('system.updates.install.reload')"
        data-testid="update-install-reload"
        @click="reload()"
      />
      <template v-else-if="offer && offer.action === 'install'">
        <UButton
          v-if="fetched?.state !== 'ready' && !installing"
          color="neutral"
          variant="outline"
          icon="i-lucide-download"
          :label="t('system.updates.modal.download')"
          :loading="fetching"
          :disabled="fetching"
          data-testid="update-download-start"
          @click="downloadNow()"
        />
        <UButton
          icon="i-lucide-refresh-cw"
          :label="ended ? t('system.updates.install.retry') : t('system.updates.modal.install')"
          :loading="starting || installing"
          :disabled="installing"
          data-testid="update-install-start"
          @click="installNow()"
        />
      </template>
      <UButton
        v-else-if="offer && offer.action === 'download'"
        icon="i-lucide-download"
        :label="t('system.updates.modal.download')"
        :to="offer.download_url ?? offer.release_url"
        target="_blank"
        rel="noopener"
      />
    </template>
  </UModal>
</template>
