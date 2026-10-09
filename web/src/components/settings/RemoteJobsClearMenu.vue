<script setup lang="ts">
/**
 * *Clear list* over the remote jobs the filters show (RD-1200-01).
 *
 * The two ways a single job ends, for the whole list: removed here only, or deleted at the
 * provider first. One menu, as on Downloads (`design.md`, *A menu holds at most one red entry*):
 * clearing the list here only tidies up jobs that have stopped and is neutral; deleting at the
 * provider reaches into the person's accounts outside this machine, is red and stands apart.
 * Both questions name how many jobs go, at which providers, and how many still-running jobs stay
 * -- the server leaves those out the same way. The request carries the filter, not ids, and
 * `confirmed: true`; the server refuses it without.
 */
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { Account, RemoteJob } from '@/api/types'
import { clearBody, clearSelection, type RemoteJobFilter } from '@/components/settings/remoteJobsFilter'
import { useAccountProviders } from '@/composables/useAccountProviders'
import { useConfirm } from '@/composables/useConfirm'
import { translateServerMessage } from '@/i18n/server'

const props = defineProps<{ visible: RemoteJob[], accounts: Account[], filter: RemoteJobFilter }>()
const emit = defineEmits<{ cleared: [], message: [string], error: [string] }>()

const { t } = useI18n()
const confirm = useConfirm()
const { providerName } = useAccountProviders()
const busy = ref(false)

const selection = computed(() => clearSelection(props.visible, props.accounts))
/** Whether any listed job names something at its provider; without one the red entry has nothing to reach. */
const reachesProvider = computed(() => selection.value.targets.some(job => job.state !== 'discarded' && job.remote_id))

const items = computed(() => {
  const count = selection.value.targets.length
  return [[
    {
      label: t('remote_jobs.clear.local.action', { count }, count),
      icon: 'i-lucide-list-x',
      disabled: !count,
      onSelect: (): void => void clear(false)
    }
  ], [
    {
      label: t('remote_jobs.clear.provider.action', { count }, count),
      icon: 'i-lucide-cloud-off',
      color: 'error' as const,
      disabled: !count || !reachesProvider.value,
      onSelect: (): void => void clear(true)
    }
  ]]
})

/** The question, built from the selection: the count, the providers, and what stays running. */
function question(atProvider: boolean): string {
  const { targets, running, providers } = selection.value
  const variant = atProvider ? 'provider' : 'local'
  const names = providers.map(providerName).join(', ') || t('remote_jobs.clear.no_provider')
  const parts = [t(`remote_jobs.clear.${variant}.confirm_description`, { count: targets.length, providers: names }, targets.length)]
  if (running.length) parts.push(t('remote_jobs.clear.running_left', { count: running.length }, running.length))
  return parts.join(' ')
}

async function clear(atProvider: boolean): Promise<void> {
  const variant = atProvider ? 'provider' : 'local'
  const confirmed = await confirm({
    title: t(`remote_jobs.clear.${variant}.confirm_title`),
    description: question(atProvider),
    confirmLabel: t(`remote_jobs.clear.${variant}.confirm_label`),
    confirmIcon: 'i-lucide-trash-2',
    destructive: true
  })
  if (!confirmed) return
  busy.value = true
  const response = await api.POST('/api/v1/remote-jobs/clear', { body: clearBody(props.filter, atProvider) })
  busy.value = false
  if (!response.data) return void emit('error', responseError(response))
  emit('cleared')
  const { removed, failed, results } = response.data
  if (!failed) return void emit('message', t('remote_jobs.clear.done', { count: removed }, removed))
  // Each job that stayed names why; the same reason from several jobs is said once.
  const reasons = new Set(results
    .filter(job => !job.removed)
    .map(job => translateServerMessage({ code: job.code ?? '', message: job.message ?? '' })))
  emit('error', [t('remote_jobs.clear.partial', { removed, failed }, failed), ...reasons].join(' '))
}
</script>

<template>
  <UDropdownMenu :items="items">
    <UButton
      icon="i-lucide-list-x"
      :label="t('remote_jobs.clear.menu')"
      color="neutral"
      variant="outline"
      size="sm"
      :loading="busy"
      :disabled="!props.visible.length"
      data-testid="remote-jobs-clear"
    />
  </UDropdownMenu>
</template>
