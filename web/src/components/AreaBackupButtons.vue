<script setup lang="ts">
import { ref } from 'vue'
import { useToast } from '@nuxt/ui/composables'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { AreaBundle } from '@/api/types'
import { useConfirm } from '@/composables/useConfirm'

/**
 * Export and import for one configuration area.
 *
 * Generalised from `routing/RoutingBackupButtons.vue` rather than copied three times: the areas
 * differ in their endpoints and in what a summary counts, and in nothing else. The file is the
 * same format throughout, so a subscriptions file dropped on the streams page is refused by the
 * server with a named reason instead of quietly importing nothing.
 */
const props = defineProps<{
  /** Path segment of the area, e.g. `subscriptions`; also the file name and the toast text. */
  area: 'subscriptions' | 'streams' | 'automations'
}>()
const emit = defineEmits<{ imported: [] }>()

const { t } = useI18n()
const toast = useToast()
const confirm = useConfirm()
const exporting = ref(false)
const importing = ref(false)
const fileInput = ref<HTMLInputElement | null>(null)

function fail(message: string): void {
  toast.add({ title: message, color: 'error', icon: 'i-lucide-circle-alert' })
}

async function exportArea(): Promise<void> {
  exporting.value = true
  // openapi-fetch types each path literally, so the three are named rather than built.
  const response = await (props.area === 'subscriptions'
    ? api.GET('/api/v1/subscriptions/export')
    : props.area === 'streams'
      ? api.GET('/api/v1/streams/export')
      : api.GET('/api/v1/automations/export'))
  exporting.value = false
  if (!response.data) return fail(responseError(response))

  const blob = new Blob([JSON.stringify(response.data, null, 2)], { type: 'application/json' })
  const url = URL.createObjectURL(blob)
  const anchor = document.createElement('a')
  anchor.href = url
  anchor.download = `rdownloader-${props.area}-${new Date().toISOString().slice(0, 10)}.json`
  document.body.append(anchor)
  anchor.click()
  anchor.remove()
  URL.revokeObjectURL(url)
  toast.add({ title: t('common.backup.export_success'), color: 'success', icon: 'i-lucide-file-check-2' })
}

function chooseFile(): void {
  if (!fileInput.value) return
  fileInput.value.value = ''
  fileInput.value.click()
}

async function selectFile(event: Event): Promise<void> {
  const target = event.target
  const file = target instanceof HTMLInputElement ? target.files?.item(0) : null
  if (!file) return
  let bundle: AreaBundle
  try {
    const parsed: unknown = JSON.parse(await file.text())
    if (!isAreaBundle(parsed)) return fail(t('common.backup.invalid_file'))
    bundle = parsed
  } catch {
    return fail(t('common.backup.invalid_file'))
  }
  const accepted = await confirm({
    title: t('common.backup.confirm_title'),
    description: t('common.backup.confirm_description', { count: entryCount(bundle) }),
    confirmLabel: t('common.backup.confirm'),
    confirmIcon: 'i-lucide-file-input'
  })
  if (!accepted) return

  importing.value = true
  const response = await (props.area === 'subscriptions'
    ? api.POST('/api/v1/subscriptions/import', { body: bundle })
    : props.area === 'streams'
      ? api.POST('/api/v1/streams/import', { body: bundle })
      : api.POST('/api/v1/automations/import', { body: bundle }))
  importing.value = false
  if (!response.data) return fail(responseError(response))

  toast.add({
    title: t('common.backup.import_success'),
    description: t('common.backup.import_summary', {
      created: response.data.created,
      skipped: response.data.skipped
    }),
    color: 'success',
    icon: 'i-lucide-file-check-2'
  })
  emit('imported')
}

/** How many entries the file carries, for the confirmation. Sections it lacks count as none. */
function entryCount(bundle: AreaBundle): number {
  return (
    (bundle.subscriptions?.length ?? 0) +
    (bundle.stream_channels?.length ?? 0) +
    (bundle.stream_schedules?.length ?? 0) +
    (bundle.automations?.length ?? 0)
  )
}

function isAreaBundle(value: unknown): value is AreaBundle {
  if (typeof value !== 'object' || value === null) return false
  const record = value as Record<string, unknown>
  return record.format === 'rdownloader-area-bundle' && record.version === 1
}
</script>

<template>
  <div class="flex items-center gap-2">
    <input ref="fileInput" class="hidden" type="file" accept=".json,application/json" @change="selectFile">
    <UButton icon="i-lucide-download" :label="t('common.backup.export')" color="neutral" variant="outline" size="sm" :loading="exporting" @click="exportArea" />
    <UButton icon="i-lucide-file-up" :label="t('common.backup.import')" color="neutral" variant="outline" size="sm" :loading="importing" @click="chooseFile" />
  </div>
</template>
