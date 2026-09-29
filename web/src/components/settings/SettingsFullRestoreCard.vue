<script setup lang="ts">
/**
 * Restoring a full backup (RD-160-03): where a restore stands, and the dialog that stages one.
 *
 * A staged restore waits for the next start; until then it can be discarded, and the running
 * installation is not touched. A restore whose first start failed was put back by the start
 * itself — the card says so, with the reason, until it is dismissed.
 */
import { computed, onMounted, ref } from 'vue'
import { useToast } from '@nuxt/ui/composables'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import { discardRestore, restoreStatus } from '@/api/fullRestore'
import type { BackupRun, RestoreStatus } from '@/api/types'
import DataState from '@/components/DataState.vue'
import SectionHeader from '@/components/SectionHeader.vue'
import FullRestoreDialog from '@/components/settings/FullRestoreDialog.vue'
import { useFetchState } from '@/composables/useFetchState'
import { formatMoment } from '@/utils/format'

const { t } = useI18n()
const toast = useToast()
const { loading, loadError, load } = useFetchState()

const status = ref<RestoreStatus | null>(null)
const runs = ref<BackupRun[]>([])
const dialogOpen = ref(false)
const discarding = ref(false)
const error = ref<string | null>(null)

const waiting = computed(() => status.value?.state === 'staged' || status.value?.state === 'switching')

async function read(): Promise<string | null> {
  const [answer, runsResponse] = await Promise.all([restoreStatus(), api.GET('/api/v1/backups/runs')])
  if (!answer.ok) return answer.error
  if (!runsResponse.data) return responseError(runsResponse)
  status.value = answer.data
  runs.value = runsResponse.data
  return null
}

async function discard(): Promise<void> {
  error.value = null
  discarding.value = true
  const answer = await discardRestore()
  discarding.value = false
  if (!answer.ok) {
    error.value = answer.error
    return
  }
  status.value = answer.data
  toast.add({ title: t('system.backup.full_restore.status.discarded'), color: 'info', icon: 'i-lucide-undo-2' })
}

async function staged(): Promise<void> {
  toast.add({ title: t('system.backup.full_restore.confirm.staged'), color: 'success', icon: 'i-lucide-archive-restore' })
  await load(read)
}

onMounted(() => void load(read))
</script>

<template>
  <section class="border border-muted bg-default p-5 xl:col-span-2" data-testid="full-restore">
    <SectionHeader
      :eyebrow="t('system.backup.full_restore.eyebrow')"
      :title="t('system.backup.full_restore.title')"
      :description="t('system.backup.full_restore.description')"
    />
    <UAlert v-if="error" class="mt-5" color="error" variant="subtle" icon="i-lucide-circle-alert" :description="error" />
    <DataState :loading="loading" :error="loadError" class="mt-5" />
    <template v-if="!loading && !loadError && status">
      <UAlert
        v-if="waiting"
        class="mt-5"
        color="info"
        variant="subtle"
        icon="i-lucide-clock"
        data-testid="full-restore-waiting"
        :description="t(`system.backup.full_restore.status.${status.state}`, { archive: status.archive_name ?? '', date: formatMoment(status.backup_created_at) })"
      />
      <UAlert
        v-else-if="status.state === 'failed'"
        class="mt-5"
        color="error"
        variant="subtle"
        icon="i-lucide-triangle-alert"
        data-testid="full-restore-failed"
        :description="t('system.backup.full_restore.status.failed', { archive: status.archive_name ?? '', date: formatMoment(status.failed_at), reason: status.reason ?? '' })"
      />
      <div class="mt-5 flex flex-wrap gap-2">
        <UButton
          icon="i-lucide-archive-restore"
          :label="t('system.backup.full_restore.open')"
          :disabled="waiting"
          data-testid="full-restore-open"
          @click="dialogOpen = true"
        />
        <UButton
          v-if="status.state === 'staged' || status.state === 'failed'"
          color="neutral"
          variant="outline"
          icon="i-lucide-x"
          :label="status.state === 'staged' ? t('system.backup.full_restore.status.discard') : t('system.backup.full_restore.status.dismiss')"
          :loading="discarding"
          data-testid="full-restore-discard"
          @click="discard"
        />
      </div>
    </template>
    <FullRestoreDialog v-model:open="dialogOpen" :runs="runs" @staged="staged" />
  </section>
</template>
