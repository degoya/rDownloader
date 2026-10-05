<script setup lang="ts">
/**
 * One server's traffic and quota on its row in the server chain (RD-1100-05).
 *
 * The usage line reads `/api/v1/stats/usenet-servers`, the quota itself comes with the server.
 * The form is folded away until somebody wants to change it: most servers have no quota, and a
 * block account's needs a number, an action and maybe a reset day, nothing more.
 */
import { computed, reactive, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { SetUsenetQuota, UsenetQuotaAction, UsenetServer, UsenetServerTraffic } from '@/api/types'
import { GIB, formatBytes, formatDay } from '@/utils/format'

const props = defineProps<{ server: UsenetServer, traffic?: UsenetServerTraffic | null | undefined }>()
const emit = defineEmits<{ saved: [server: UsenetServer] }>()

const { t } = useI18n()
const open = ref(false)
const pending = ref(false)
const error = ref<string | null>(null)
const form = reactive<{ limitGiB: number | null, action: UsenetQuotaAction, resetOn: string }>({
  limitGiB: null,
  action: 'backup',
  resetOn: ''
})

const quota = computed(() => props.server.quota ?? null)
const percent = computed(() => {
  const current = quota.value
  if (!current || current.limit_bytes <= 0) return 0
  return Math.min(100, Math.round((current.used_bytes / current.limit_bytes) * 100))
})
const actionItems = computed(() => [
  { label: t('usenet.quota.action_backup'), value: 'backup' },
  { label: t('usenet.quota.action_pause'), value: 'pause' }
])

function edit(): void {
  const current = quota.value
  form.limitGiB = current ? Math.round((current.limit_bytes / GIB) * 100) / 100 : null
  form.action = current?.action ?? 'backup'
  form.resetOn = current?.reset_on ?? ''
  error.value = null
  open.value = true
}

async function send(body: SetUsenetQuota): Promise<void> {
  pending.value = true
  error.value = null
  const response = await api.PUT('/api/v1/usenet/servers/{id}/quota', {
    params: { path: { id: props.server.id } },
    body
  })
  pending.value = false
  if (!response.data) {
    error.value = responseError(response)
    return
  }
  open.value = false
  emit('saved', response.data)
}

function current(): SetUsenetQuota {
  const stored = quota.value
  return {
    limit_bytes: stored?.limit_bytes ?? null,
    action: stored?.action ?? 'backup',
    reset_on: stored?.reset_on ?? null
  }
}

function save(): Promise<void> {
  const limit = Number(form.limitGiB)
  return send({
    limit_bytes: Number.isFinite(limit) && limit > 0 ? Math.round(limit * GIB) : 0,
    action: form.action,
    reset_on: form.resetOn || null
  })
}

const resetUsage = () => send({ ...current(), reset_usage: true })
const remove = () => send({ limit_bytes: null })
</script>

<template>
  <div class="mt-3 space-y-2 text-xs" :data-quota="server.id">
    <p v-if="traffic" class="text-muted">
      {{ t('usenet.quota.usage', { today: formatBytes(String(traffic.today)), month: formatBytes(String(traffic.month)), total: formatBytes(String(traffic.total)) }) }}
    </p>
    <div class="flex flex-wrap items-center gap-2">
      <span class="font-medium text-highlighted">{{ t('usenet.quota.title') }}</span>
      <span v-if="quota" class="numeric text-muted">{{ t('usenet.quota.used', { used: formatBytes(String(quota.used_bytes)), limit: formatBytes(String(quota.limit_bytes)) }) }}</span>
      <span v-else class="text-muted">{{ t('usenet.quota.none') }}</span>
      <span v-if="quota?.reset_on" class="text-muted">· {{ t('usenet.quota.resets_on', { date: formatDay(`${quota.reset_on}T12:00:00Z`) }) }}</span>
      <UButton v-if="!open" size="xs" color="neutral" variant="ghost" icon="i-lucide-gauge" :label="t('usenet.quota.edit')" @click="edit" />
    </div>
    <UProgress v-if="quota" :model-value="percent" size="xs" :color="quota.reached_at ? 'warning' : 'primary'" />
    <p v-if="quota?.reached_at" class="text-warning" role="status">
      {{ quota.action === 'pause' ? t('usenet.quota.reached_pause') : t('usenet.quota.reached_backup') }}
    </p>
    <form v-if="open" class="grid gap-3 border border-muted p-3" @submit.prevent="save">
      <UAlert v-if="error" color="error" variant="subtle" :description="error" />
      <UFormField :label="t('usenet.quota.limit')" name="quota_limit" :description="t('usenet.quota.limit_hint')" required>
        <UInput v-model.number="form.limitGiB" type="number" min="0.01" step="any" required class="w-full">
          <template #trailing><span class="font-mono text-xs text-muted">GiB</span></template>
        </UInput>
      </UFormField>
      <UFormField :label="t('usenet.quota.action')" name="quota_action">
        <USelect v-model="form.action" :items="actionItems" class="w-full" />
      </UFormField>
      <UFormField :label="t('usenet.quota.reset_on')" name="quota_reset_on" :description="t('usenet.quota.reset_on_hint')">
        <UInput v-model="form.resetOn" type="date" class="w-full" />
      </UFormField>
      <div class="flex flex-wrap gap-2">
        <UButton type="submit" size="sm" icon="i-lucide-save" :label="t('common.actions.save')" :loading="pending" />
        <UButton v-if="quota" size="sm" color="neutral" variant="subtle" icon="i-lucide-rotate-ccw" :label="t('usenet.quota.reset_usage')" :disabled="pending" @click="resetUsage" />
        <UButton v-if="quota" size="sm" color="error" variant="ghost" icon="i-lucide-x" :label="t('usenet.quota.remove')" :disabled="pending" @click="remove" />
        <UButton size="sm" color="neutral" variant="ghost" :label="t('common.actions.cancel')" :disabled="pending" @click="open = false" />
      </div>
    </form>
  </div>
</template>
