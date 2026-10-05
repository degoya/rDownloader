<script setup lang="ts">
import { computed, ref } from 'vue'
import { useToast } from '@nuxt/ui/composables'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { RoutingBundle } from '@/api/types'
import { useConfirm } from '@/composables/useConfirm'
import { useErrorToast } from '@/composables/useErrorToast'
import { JsonRefusal, useJsonImport } from '@/composables/useJsonImport'
import { downloadJson } from '@/utils/jsonFile'

const emit = defineEmits<{ imported: [] }>()
const { t } = useI18n()
const toast = useToast()
const showError = useErrorToast()
const confirm = useConfirm()
const exporting = ref(false)
const importing = ref(false)

type ExportPart = 'all' | 'categories' | 'rules'

/** Everything, or one part on its own: rules can travel without their categories and back. */
const exportItems = computed(() => [
  { label: t('routing.backup.export_all'), icon: 'i-lucide-layers', onSelect: () => exportRouting('all') },
  { label: t('routing.backup.export_categories'), icon: 'i-lucide-folder-tree', onSelect: () => exportRouting('categories') },
  { label: t('routing.backup.export_rules'), icon: 'i-lucide-list-filter', onSelect: () => exportRouting('rules') }
])

async function exportRouting(part: ExportPart): Promise<void> {
  exporting.value = true
  const response = await api.GET('/api/v1/routing/export', { params: { query: { part } } })
  exporting.value = false
  if (!response.data) return fail(responseError(response))
  const suffix = part === 'all' ? '' : `-${part}`
  downloadJson(response.data, `routing${suffix}`)
  toast.add({ title: t('routing.backup.export_success'), color: 'success', icon: 'i-lucide-file-check-2' })
}

function fail(message: string): void {
  showError(message)
}

/** The same import as `AreaBackupButtons`, in the routing bundle's own format (WEB-09). */
const { select: selectFile } = useJsonImport<RoutingBundle>({
  check: parsed => isRoutingBundle(parsed) ? parsed : new JsonRefusal(t('routing.backup.invalid_file')),
  unreadable: () => t('routing.backup.invalid_file'),
  refuse: fail,
  take: importRouting
})

async function importRouting(bundle: RoutingBundle): Promise<void> {
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
  if (!response.data) return fail(responseError(response))
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
    <UDropdownMenu :items="exportItems">
      <UButton icon="i-lucide-download" trailing-icon="i-lucide-chevron-down" :label="t('routing.backup.export')" color="neutral" variant="outline" size="sm" :loading="exporting" />
    </UDropdownMenu>
    <UFileUpload v-slot="{ open }" :model-value="null" accept=".json" reset :dropzone="false" @update:model-value="selectFile">
      <UButton icon="i-lucide-file-up" :label="t('routing.backup.import')" color="neutral" variant="outline" size="sm" :loading="importing" @click="open()" />
    </UFileUpload>
  </div>
</template>
