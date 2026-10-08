<script setup lang="ts">
/**
 * "Verify with passphrase" for one destination (RD-1190-19). The verification opens an archive
 * with the key the schedule keeps; a restore takes only the passphrase, and a passphrase that is
 * lost cannot be recovered. This opens the destination's newest archive with a typed passphrase
 * the way a restore preview does — reading it whole, writing nothing — so "verified" can also
 * mean "restorable by whoever knows the passphrase".
 *
 * The passphrase goes into the one request and is cleared when the dialog closes.
 */
import { ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import { previewRestore } from '@/api/fullRestore'
import { formatMoment } from '@/utils/format'

const props = defineProps<{ destinationId: string, disabled?: boolean }>()

const { t } = useI18n()

const open = ref(false)
const passphrase = ref('')
const checking = ref(false)
const error = ref<string | null>(null)
const opened = ref<string | null>(null)

watch(open, (value) => {
  if (value) return
  passphrase.value = ''
  error.value = null
  opened.value = null
})

async function check(): Promise<void> {
  error.value = null
  opened.value = null
  checking.value = true
  const archives = await api.GET('/api/v1/backups/archives', { params: { query: { destination_id: props.destinationId } } })
  const newest = archives.data?.[0]
  if (!newest) {
    checking.value = false
    error.value = archives.data ? t('system.backup.full.destinations.verify_none') : responseError(archives)
    return
  }
  const answer = await previewRestore({ run_id: newest.run_id }, passphrase.value)
  checking.value = false
  if (!answer.ok) {
    error.value = answer.error
    return
  }
  opened.value = t('system.backup.full.destinations.passphrase_check.ok', {
    name: answer.data.archive_name,
    date: formatMoment(answer.data.created_at)
  })
}
</script>

<template>
  <UButton
    size="xs"
    color="neutral"
    variant="outline"
    icon="i-lucide-key-round"
    :label="t('system.backup.full.destinations.passphrase_check.action')"
    :disabled="props.disabled"
    data-testid="backup-passphrase-check"
    @click="open = true"
  />
  <UModal
    v-model:open="open"
    :title="t('system.backup.full.destinations.passphrase_check.title')"
    :description="t('system.backup.full.destinations.passphrase_check.description')"
  >
    <template #body>
      <form class="space-y-3" data-testid="backup-passphrase-check-form" @submit.prevent="check">
        <UAlert v-if="error" color="error" icon="i-lucide-circle-alert" :description="error" />
        <UAlert v-if="opened" color="success" icon="i-lucide-circle-check" :description="opened" />
        <UFormField
          name="backup-passphrase-check"
          :label="t('system.backup.full_restore.passphrase.label')"
          :description="t('system.backup.full.destinations.passphrase_check.hint')"
          required
        >
          <UInput v-model="passphrase" type="password" autocomplete="off" class="w-full" data-testid="backup-passphrase-check-input" />
        </UFormField>
        <UButton
          type="submit"
          icon="i-lucide-shield-check"
          :label="t('system.backup.full.destinations.passphrase_check.submit')"
          :disabled="passphrase.length === 0"
          :loading="checking"
          data-testid="backup-passphrase-check-submit"
        />
      </form>
    </template>
  </UModal>
</template>
