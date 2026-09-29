<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import {
  checkContentIndex,
  listLinkSupport,
  listReuseCapabilities,
  listStorageOperations,
  type LinkSupport,
  type ReuseCapability,
  type RunnerReuse,
  type StorageOperation
} from '@/api/storage'
import SectionHeader from '@/components/SectionHeader.vue'
import { translateServerMessage } from '@/i18n/server'
import { formatBytes, formatMoment } from '@/utils/format'

/**
 * What the storage layer did and can do (RD-150-02): the history of verified moves and dedupe
 * links, which roots take hard links, what each transfer kind reuses of data on disk, and a
 * check of the content index against the disk.
 */
const { t } = useI18n()
const operations = ref<StorageOperation[]>([])
const reuse = ref<RunnerReuse[]>([])
const links = ref<LinkSupport[]>([])
const error = ref<string | null>(null)
const checkResult = ref<string | null>(null)
const checking = ref(false)

const REUSE_FIELDS: (keyof ReuseCapability)[] = [
  'resume_partial',
  'recheck_partial',
  'adopt_completed',
  'verify_completed',
  'applies_collision_policy'
]

async function load(): Promise<void> {
  const [history, capabilities, support] = await Promise.all([
    listStorageOperations(50),
    listReuseCapabilities(),
    listLinkSupport()
  ])
  if (history.ok) operations.value = history.data
  if (capabilities.ok) reuse.value = capabilities.data
  if (support.ok) links.value = support.data
  const failed = [history, capabilities, support].find(answer => !answer.ok)
  error.value = failed && !failed.ok ? translateServerMessage(failed.message) : null
}

async function check(): Promise<void> {
  checking.value = true
  checkResult.value = null
  const answer = await checkContentIndex()
  checking.value = false
  if (!answer.ok) {
    error.value = translateServerMessage(answer.message)
    return
  }
  checkResult.value = t('settings.storage.activity.index_checked', { ...answer.data })
}

function stateColor(state: StorageOperation['state']): 'success' | 'error' | 'warning' | 'neutral' {
  if (state === 'completed') return 'success'
  if (state === 'failed') return 'error'
  if (state === 'interrupted') return 'warning'
  return 'neutral'
}

onMounted(() => void load())
</script>

<template>
  <section data-settings-anchor="routing.storage_activity" class="space-y-4 border border-muted bg-default p-5" data-testid="storage-activity">
    <SectionHeader
      :eyebrow="t('settings.storage.activity.eyebrow')"
      :title="t('settings.storage.activity.title')"
      :description="t('settings.storage.activity.description')"
    />
    <UAlert v-if="error" color="error" variant="subtle" icon="i-lucide-circle-alert" :description="error" />
    <UAlert v-if="checkResult" color="success" variant="subtle" icon="i-lucide-circle-check" :description="checkResult" />

    <div>
      <p class="text-sm font-medium text-highlighted">{{ t('settings.storage.activity.links_title') }}</p>
      <p class="mt-1 text-xs leading-5 text-muted">{{ t('settings.storage.activity.links_description') }}</p>
      <ul class="mt-2 space-y-1 text-xs">
        <li v-for="entry in links" :key="entry.path" class="flex items-center gap-2">
          <UBadge :color="entry.hardlink ? 'success' : 'neutral'" variant="subtle" size="sm" :label="entry.hardlink ? t('settings.storage.activity.hardlink_yes') : t('settings.storage.activity.hardlink_no')" />
          <span class="font-medium">{{ entry.name }}</span>
          <span class="truncate font-mono text-muted">{{ entry.path }}</span>
        </li>
      </ul>
    </div>

    <div>
      <p class="text-sm font-medium text-highlighted">{{ t('settings.storage.activity.reuse_title') }}</p>
      <p class="mt-1 text-xs leading-5 text-muted">{{ t('settings.storage.activity.reuse_description') }}</p>
      <div class="mt-2 overflow-x-auto">
        <table class="w-full text-xs">
          <thead>
            <tr class="text-left text-muted">
              <th class="py-1 pr-3 font-normal">{{ t('settings.storage.activity.kind') }}</th>
              <th v-for="field in REUSE_FIELDS" :key="field" class="py-1 pr-3 font-normal">{{ t(`settings.storage.activity.reuse.${field}`) }}</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="entry in reuse" :key="entry.kind" class="border-t border-muted">
              <td class="py-1 pr-3 font-medium">{{ t(`settings.storage.activity.runner.${entry.kind}`) }}</td>
              <td v-for="field in REUSE_FIELDS" :key="field" class="py-1 pr-3">
                <UIcon :name="entry.capability[field] ? 'i-lucide-check' : 'i-lucide-minus'" :class="entry.capability[field] ? 'text-success' : 'text-muted'" :aria-label="entry.capability[field] ? t('settings.storage.activity.yes') : t('settings.storage.activity.no')" />
              </td>
            </tr>
          </tbody>
        </table>
      </div>
    </div>

    <div>
      <div class="flex flex-wrap items-center justify-between gap-3">
        <div>
          <p class="text-sm font-medium text-highlighted">{{ t('settings.storage.activity.history_title') }}</p>
          <p class="mt-1 text-xs leading-5 text-muted">{{ t('settings.storage.activity.history_description') }}</p>
        </div>
        <UButton size="xs" color="neutral" variant="outline" icon="i-lucide-scan-search" :label="t('settings.storage.activity.check_index')" :loading="checking" @click="check" />
      </div>
      <p v-if="!operations.length" class="mt-2 text-xs text-muted">{{ t('settings.storage.activity.history_empty') }}</p>
      <ul v-else class="mt-2 space-y-2 text-xs" data-testid="storage-history">
        <li v-for="operation in operations" :key="operation.id" class="border-t border-muted pt-2">
          <div class="flex flex-wrap items-center gap-2">
            <UBadge :color="stateColor(operation.state)" variant="subtle" size="sm" :label="t(`settings.storage.activity.states.${operation.state}`)" />
            <span class="font-medium">{{ t(`settings.storage.activity.operation.${operation.kind}`) }}</span>
            <span class="text-muted">{{ formatMoment(operation.started_at) }}</span>
            <span v-if="operation.size_bytes !== null" class="text-muted">{{ formatBytes(String(operation.size_bytes)) }}</span>
            <UBadge v-if="operation.verified_digest" color="success" variant="outline" size="sm" icon="i-lucide-shield-check" :label="t('settings.storage.activity.verified')" :title="operation.verified_digest" />
          </div>
          <p class="mt-1 truncate font-mono text-muted" :title="`${operation.source_path} → ${operation.target_path}`">{{ operation.source_path }} → {{ operation.target_path }}</p>
          <p v-if="operation.error_code" class="mt-1 text-error">{{ translateServerMessage({ code: operation.error_code, message: operation.error_message ?? '' }) }}</p>
        </li>
      </ul>
    </div>
  </section>
</template>
