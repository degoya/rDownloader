<script setup lang="ts">
import { ref } from 'vue'
import { useToast } from '@nuxt/ui/composables'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { RoutingBundle } from '@/api/types'
import { useConfirm } from '@/composables/useConfirm'

const emit = defineEmits<{ imported: [] }>()
const { t } = useI18n()
const toast = useToast()
const confirm = useConfirm()
const exporting = ref(false)
const importing = ref(false)
const fileInput = ref<HTMLInputElement | null>(null)

async function exportRouting(): Promise<void> {
  exporting.value = true
  const response = await api.GET('/api/v1/routing/export')
  exporting.value = false
  if (!response.data) {
    toast.add({ title: responseError(response), color: 'error', icon: 'i-lucide-circle-alert' })
    return
  }
  const blob = new Blob([JSON.stringify(response.data, null, 2)], { type: 'application/json' })
  const url = URL.createObjectURL(blob)
  const anchor = document.createElement('a')
  anchor.href = url
  anchor.download = `rdownloader-routing-${new Date().toISOString().slice(0, 10)}.json`
  document.body.append(anchor)
  anchor.click()
  anchor.remove()
  URL.revokeObjectURL(url)
  toast.add({ title: t('routing.backup.export_success'), color: 'success', icon: 'i-lucide-file-check-2' })
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
  let bundle: RoutingBundle
  try {
    const parsed: unknown = JSON.parse(await file.text())
    if (!isRoutingBundle(parsed)) {
      toast.add({ title: t('routing.backup.invalid_file'), color: 'error', icon: 'i-lucide-circle-alert' })
      return
    }
    bundle = parsed
  } catch {
    toast.add({ title: t('routing.backup.invalid_file'), color: 'error', icon: 'i-lucide-circle-alert' })
    return
  }
  const accepted = await confirm({
    title: t('routing.backup.confirm_title'),
    description: t('routing.backup.confirm_description', {
      categories: bundle.categories?.length ?? 0,
      rules: bundle.rules?.length ?? 0
    }),
    confirmLabel: t('routing.backup.confirm'),
    confirmIcon: 'i-lucide-file-input'
  })
  if (!accepted) return
  importing.value = true
  const response = await api.POST('/api/v1/routing/import', { body: bundle })
  importing.value = false
  if (!response.data) {
    toast.add({ title: responseError(response), color: 'error', icon: 'i-lucide-circle-alert' })
    return
  }
  const summary = response.data
  toast.add({
    title: t('routing.backup.import_success'),
    description: t('routing.backup.import_summary', {
      categories: summary.categories_created,
      rules: summary.rules_created,
      skipped: summary.categories_skipped + summary.rules_skipped
    }),
    color: 'success',
    icon: 'i-lucide-file-check-2'
  })
  emit('imported')
}

function isRoutingBundle(value: unknown): value is RoutingBundle {
  if (typeof value !== 'object' || value === null) return false
  const record = value as Record<string, unknown>
  return record.format === 'rdownloader-routing-bundle' && record.version === 1
}
</script>

<template>
  <div class="flex items-center gap-2">
    <input ref="fileInput" class="hidden" type="file" accept=".json,application/json" @change="selectFile">
    <UButton icon="i-lucide-download" :label="t('routing.backup.export')" color="neutral" variant="outline" size="sm" :loading="exporting" @click="exportRouting" />
    <UButton icon="i-lucide-file-up" :label="t('routing.backup.import')" color="neutral" variant="outline" size="sm" :loading="importing" @click="chooseFile" />
  </div>
</template>
