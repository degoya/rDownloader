<script setup lang="ts">
/**
 * What the data folder holds for taking updates back and what the plugin cache holds, and the
 * clean-up of both (RD-1240-34), and the database beside them (RD-1240-35): its events, the
 * subscription archive whose old skipped or dismissed entries keep only their key, and the free
 * pages the file can hand back. The service runs the same clean-up ten minutes after every start
 * and daily; the button is for somebody who wants the space now, and is the only way a database
 * from before 1.24 is rewritten once — the service answers in `rewrite_refused` why not now.
 *
 * Like the data-reset buttons it names the amount in the question and sends the confirmation as
 * a value (`confirmed: true`), and it is disabled while there is nothing to remove. The days the
 * newest copy stays and the days an archived entry stays whole are fields of the settings
 * document, saved with the page.
 */
import { computed, onMounted, ref } from 'vue'
import { useToast } from '@nuxt/ui/composables'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import { translateServerMessage } from '@/i18n/server'
import type { CleanupSummary, Settings } from '@/api/types'
import NumberWithUnit from '@/components/NumberWithUnit.vue'
import SectionHeader from '@/components/SectionHeader.vue'
import StatTiles from '@/components/StatTiles.vue'
import { useConfirm } from '@/composables/useConfirm'
import { formatBytes } from '@/utils/format'
import { WHOLE } from '@/utils/numberInput'

const settings = defineModel<Settings>({ required: true })
const { t, n } = useI18n()
const confirm = useConfirm()
const toast = useToast()
const summary = ref<CleanupSummary | null>(null)
const busy = ref(false)
const error = ref<string | null>(null)

onMounted(() => { void load() })

async function load(): Promise<void> {
  const response = await api.GET('/api/v1/system/cleanup')
  // Only a whole answer: the tiles read all three stores.
  if (response.data?.pre_update) summary.value = response.data
}

const size = (bytes: number): string => formatBytes(BigInt(bytes))

/** Every byte a clean-up removes or gives back, the database file's included. */
function freed(value: CleanupSummary): number {
  return value.pre_update.removable_bytes + value.pre_migration.removable_bytes + value.plugin_cache.removable_bytes + value.database.removable_bytes
}

const removable = computed(() => summary.value ? freed(summary.value) : 0)

/** Something to do even when no byte is promised: old archive entries, or a file to rewrite once. */
const actionable = computed(() => {
  const database = summary.value?.database
  return removable.value > 0 || (database?.compactable_items ?? 0) > 0 || (database !== undefined && !database.incremental && !database.rewrite_refused)
})

const tiles = computed(() => {
  const value = summary.value
  if (!value) return []
  return (['pre_update', 'pre_migration', 'plugin_cache'] as const).map(key => ({
    key,
    label: t(`settings.cleanup.${key}`),
    value: size(value[key].kept_bytes + value[key].removable_bytes),
    hint: t('settings.cleanup.removable', { size: size(value[key].removable_bytes) })
  }))
})

const databaseTiles = computed(() => {
  const database = summary.value?.database
  if (!database) return []
  return [
    { key: 'events', label: t('settings.cleanup.events'), value: size(database.event_bytes), hint: t('settings.cleanup.rows', { count: n(database.event_rows) }) },
    { key: 'items', label: t('settings.cleanup.items'), value: size(database.item_bytes), hint: t('settings.cleanup.compactable', { count: n(database.compactable_items) }) },
    { key: 'database', label: t('settings.cleanup.database_file'), value: size(database.file_bytes), hint: t('settings.cleanup.removable', { size: size(database.removable_bytes) }) }
  ]
})

/** Why the file is not rewritten although it would need it, translated; empty otherwise. */
const rewriteRefused = computed(() => {
  const code = summary.value?.database.rewrite_refused
  return code ? translateServerMessage({ code, message: code }) : ''
})

async function cleanUp(): Promise<void> {
  const confirmed = await confirm({
    title: t('settings.cleanup.confirm_title'),
    description: t('settings.cleanup.confirm_description', { size: size(removable.value) }),
    confirmLabel: t('settings.cleanup.confirm'),
    confirmIcon: 'i-lucide-brush-cleaning',
    destructive: true
  })
  if (!confirmed) return
  busy.value = true
  error.value = null
  const response = await api.POST('/api/v1/system/cleanup', { body: { confirmed: true } })
  busy.value = false
  if (!response.data) {
    error.value = responseError(response)
    return
  }
  toast.add({ title: t('settings.cleanup.done', { size: size(freed(response.data)) }), color: 'success', icon: 'i-lucide-brush-cleaning' })
  await load()
  // The answer's own refusal, which the fresh preview may no longer carry (the download ended).
  if (response.data.database.rewrite_refused && summary.value) summary.value.database.rewrite_refused = response.data.database.rewrite_refused
}
</script>

<template>
  <UCard as="section" data-settings-anchor="system.cleanup" data-testid="update-backup-cleanup">
    <div class="flex flex-wrap items-start justify-between gap-4">
      <SectionHeader
        :eyebrow="t('settings.cleanup.eyebrow')"
        :title="t('settings.cleanup.title')"
        :description="t('settings.cleanup.description')"
      />
      <UButton
        icon="i-lucide-brush-cleaning"
        color="error"
        variant="soft"
        :label="t('settings.cleanup.button')"
        :loading="busy"
        :disabled="!actionable"
        data-testid="cleanup-run"
        @click="cleanUp()"
      />
    </div>
    <UAlert
      v-if="summary && !summary.update_proven"
      class="mt-4"
      color="warning"
      variant="subtle"
      icon="i-lucide-shield-alert"
      :description="t('settings.cleanup.unproven')"
      data-testid="cleanup-unproven"
    />
    <UAlert
      v-if="summary && !summary.database.incremental && !rewriteRefused"
      class="mt-4"
      color="info"
      variant="subtle"
      icon="i-lucide-database"
      :description="t('settings.cleanup.rewrite')"
      data-testid="cleanup-rewrite"
    />
    <UAlert
      v-if="rewriteRefused"
      class="mt-4"
      color="warning"
      variant="subtle"
      icon="i-lucide-database"
      :description="rewriteRefused"
      data-testid="cleanup-rewrite-refused"
    />
    <StatTiles v-if="tiles.length" :tiles="tiles" class="mt-4 md:grid-cols-3" data-testid="cleanup-sizes" />
    <StatTiles v-if="databaseTiles.length" :tiles="databaseTiles" class="mt-2 md:grid-cols-3" data-testid="cleanup-database" />
    <p v-if="error" class="mt-2 text-xs text-error" data-testid="cleanup-error">{{ error }}</p>
    <UFormField class="mt-4" :label="t('settings.cleanup.days_label')" :description="t('settings.cleanup.days_description')">
      <NumberWithUnit v-model="settings.update_backup_retention_days" unit="d" required :min="0" :max="3650" :format-options="WHOLE" class="mt-2 w-full" data-testid="cleanup-days" />
    </UFormField>
    <UFormField class="mt-4" :label="t('settings.cleanup.items_days_label')" :description="t('settings.cleanup.items_days_description')">
      <NumberWithUnit v-model="settings.subscription_item_retention_days" unit="d" required :min="0" :max="3650" :format-options="WHOLE" class="mt-2 w-full" data-testid="cleanup-item-days" />
    </UFormField>
  </UCard>
</template>
