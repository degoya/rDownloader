<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { InstallKind, UpdateOffer } from '@/api/updates'
import CopyField from '@/components/CopyField.vue'
import { useUpdateActions } from '@/composables/useUpdateActions'
import { formatBytes, formatLongMoment } from '@/utils/format'
import { updateHighlights } from '@/utils/updateHighlights'

/**
 * The offered version at the top of the update card (RD-1150-01): until then a small badge with
 * a "Details" link nobody took for a button (owner, 2026-10-07). Version, release date, the first
 * points of the notes and what to do — by how this installation is installed, with the dialog's
 * rules (`useUpdateActions`): "Install and restart" behind its confirmation and the background
 * "Download" for an installation that installs itself, the download for one replaced by hand,
 * the command for a package manager's, and "What's new" for the dialog with the notes. A started
 * install opens the dialog, which follows it through the restart and offers the reload.
 */
const props = defineProps<{ offer: UpdateOffer, kind: InstallKind }>()
const emit = defineEmits<{ details: [] }>()
const { t } = useI18n()
const {
  starting, installing, ended, refusal,
  fetched, fetching, fetchedPercent, fetchedReason, downloadFailure,
  installNow, downloadNow
} = useUpdateActions(() => props.offer)

const highlights = computed(() => updateHighlights(props.offer))
const kindLabel = computed(() => t(`system.updates.kind.${props.kind}`))

async function installAndFollow(): Promise<void> {
  if (await installNow()) emit('details')
}
</script>

<template>
  <UAlert
    color="primary"
    variant="subtle"
    icon="i-lucide-sparkles"
    :ui="{ title: 'text-base font-semibold', description: 'opacity-100' }"
    data-testid="update-available"
  >
    <template #title>{{ t('system.updates.available', { version: offer.version }) }}</template>
    <template #description>
      <p>{{ t('system.updates.modal.released', { when: formatLongMoment(offer.released_at) }) }}</p>
      <ul v-if="highlights.length" class="mt-2 list-disc space-y-1 ps-5 text-toned" data-testid="update-highlights">
        <li v-for="point in highlights" :key="point" class="break-words">{{ point }}</li>
      </ul>
      <div v-if="offer.action === 'command' && offer.command" class="mt-3" data-testid="update-notice-command">
        <p class="text-toned">{{ t('system.updates.modal.command_hint', { kind: kindLabel }) }}</p>
        <CopyField class="mt-2" :value="offer.command" :label="t('system.updates.modal.copy')" icon-only />
      </div>
      <p v-if="offer.action === 'install' && offer.rollback_available === false" class="mt-2 text-xs text-warning">
        {{ t('system.updates.modal.rollback_unavailable') }}
      </p>
      <div v-if="fetched && !installing" class="mt-3" data-testid="update-notice-download-progress" aria-live="polite">
        <div v-if="fetched.state === 'downloading'" class="flex flex-wrap items-center gap-2">
          <UProgress :model-value="fetchedPercent" size="xs" class="min-w-24 flex-1" />
          <span class="shrink-0 text-xs text-toned tabular-nums">
            {{ t('system.updates.download.progress', { received: formatBytes(String(fetched.received_bytes)), total: formatBytes(String(fetched.total_bytes)) }) }}
          </span>
        </div>
        <p v-else-if="fetched.state === 'ready'" class="text-success">{{ t('system.updates.download.ready', { version: fetched.version }) }}</p>
        <p v-else class="text-error">
          {{ t('system.updates.download.failed', { version: fetched.version }) }}
          <template v-if="fetchedReason"> {{ fetchedReason }}</template>
        </p>
      </div>
      <p v-else-if="downloadFailure" class="mt-3 text-error">{{ fetchedReason }}</p>
      <p v-if="refusal" class="mt-2 text-error" data-testid="update-notice-refused">{{ refusal }}</p>
    </template>
    <template #actions>
      <div class="flex flex-wrap gap-2">
        <template v-if="offer.action === 'install'">
          <UButton
            icon="i-lucide-refresh-cw"
            :label="ended ? t('system.updates.install.retry') : t('system.updates.modal.install')"
            :loading="starting || installing"
            :disabled="installing"
            data-testid="update-notice-install"
            @click="installAndFollow()"
          />
          <UButton
            v-if="fetched?.state !== 'ready' && !installing"
            color="neutral"
            variant="outline"
            icon="i-lucide-download"
            :label="t('system.updates.modal.download')"
            :loading="fetching"
            :disabled="fetching"
            data-testid="update-notice-download"
            @click="downloadNow()"
          />
        </template>
        <UButton
          v-else-if="offer.action === 'download'"
          icon="i-lucide-download"
          :label="t('system.updates.modal.download')"
          :to="offer.download_url ?? offer.release_url"
          target="_blank"
          rel="noopener"
          data-testid="update-notice-download"
        />
        <UButton
          color="neutral"
          variant="outline"
          icon="i-lucide-newspaper"
          :label="t('system.updates.whats_new')"
          data-testid="update-whats-new"
          @click="emit('details')"
        />
      </div>
    </template>
  </UAlert>
</template>
