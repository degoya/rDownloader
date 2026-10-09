<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Category, DownloadPriority } from '@/api/types'
import SearchableSelect from '@/components/SearchableSelect.vue'
import { filterImportFiles, importPackageNameOf } from '@/composables/nzbImportRequest'
import type { FileImportEntry, FileImportInput } from '@/composables/useNzbImportModal'
import { priorityItems } from '@/utils/format'
import { NO_SELECTION } from '@/utils/select'

const props = defineProps<{ categories: Category[], initialFiles?: File[] }>()
const emit = defineEmits<{ close: [result: FileImportInput | null] }>()

const { t } = useI18n()
const PRIORITY_ITEMS = computed(() => priorityItems())

const files = ref<File[]>([])
const name = ref('')
const category = ref<string>(NO_SELECTION)
const priority = ref<DownloadPriority>('normal')
const passphrase = ref('')
const enqueue = ref(false)
/** A link file may be sealed; a container may be queued once checked (RD-1210-01). */
const hasLinkFile = computed(() => files.value.some(file => /\.rdlinks$/i.test(file.name)))
const hasContainer = computed(() => files.value.some(file => /\.(?:dlc|ccf|rsdf|txt|text|rdlinks)$/i.test(file.name)))

const categoryItems = computed(() => [
  { label: t('linkgrabber.nzb.modal.no_category'), value: NO_SELECTION },
  ...props.categories.map(item => ({ label: item.name, value: item.id }))
])

function passwordOf(file: File): string | null {
  return file.name.match(/\{\{(.+)\}\}/)?.[1]?.trim() || null
}

// The marker is re-attached because the server reads the archive password from the name it stores.
function withPassword(base: string, file: File): string {
  const trimmed = base.trim()
  const password = passwordOf(file)
  return trimmed ? `${trimmed}${password ? `{{${password}}}` : ''}` : ''
}

/** New selections append (deduped by name+size); existing entries are never replaced. */
function addFiles(incoming: File[]): void {
  const matched = filterImportFiles(incoming)
  if (!matched.length) return
  const seen = new Set(files.value.map(file => `${file.name}:${file.size}`))
  const next = [...files.value]
  for (const file of matched) {
    const key = `${file.name}:${file.size}`
    if (seen.has(key)) continue
    seen.add(key)
    next.push(file)
  }
  files.value = next
}

function removeFile(index: number): void {
  files.value = files.value.filter((_, i) => i !== index)
}

// Mirrors the previous single-file behaviour: picking one file suggests its package name.
watch(files, (list) => {
  if (list.length === 1) name.value = importPackageNameOf(list[0]!.name)
}, { immediate: true })

addFiles(props.initialFiles ?? [])

function submit(): void {
  if (!files.value.length) return
  const entries: FileImportEntry[] = files.value.length === 1
    ? [{ file: files.value[0]!, name: withPassword(name.value, files.value[0]!) }]
    : files.value.map(file => ({ file, name: withPassword(importPackageNameOf(file.name), file) }))
  emit('close', {
    entries,
    categoryId: category.value === NO_SELECTION ? null : category.value,
    priority: priority.value,
    ...(hasLinkFile.value && passphrase.value ? { passphrase: passphrase.value } : {}),
    ...(hasContainer.value && enqueue.value ? { enqueue: true } : {})
  })
}
</script>

<template>
  <UModal :title="t('linkgrabber.nzb.modal.title')" :description="t('linkgrabber.nzb.modal.description')" :close="{ onClick: () => emit('close', null) }">
    <template #body>
      <form id="nzb-import-form" class="space-y-3" @submit.prevent="submit">
        <!--
          Dropped and chosen files go through `addFiles`, which keeps what `filterImportFiles`
          matches; `accept` names the extensions alone. While this modal is open the window drop
          zone (useNzbDropZone) stands back, so a drop here is taken once.
        -->
        <UFileUpload
          :model-value="[]"
          multiple
          accept=".nzb,.torrent,.dlc,.ccf,.rsdf,.txt,.rdlinks"
          icon="i-lucide-file-archive"
          :label="files.length ? t('linkgrabber.nzb.modal.drop_hint_multi') : t('linkgrabber.nzb.modal.drop_hint')"
          :interactive="false"
          :preview="false"
          class="min-h-40"
          @update:model-value="(picked: File[] | null | undefined) => addFiles(picked ?? [])"
        >
          <template #actions="{ open }">
            <UButton color="neutral" variant="outline" icon="i-lucide-folder-open" :label="t('linkgrabber.nzb.modal.choose_file')" @click="open()" />
          </template>
        </UFileUpload>
        <div v-if="files.length" class="space-y-2">
          <p class="text-sm font-medium text-highlighted">{{ t('linkgrabber.nzb.modal.files_selected', { count: files.length }, files.length) }}</p>
          <ul class="max-h-40 space-y-1 overflow-y-auto">
            <li v-for="(item, index) in files" :key="`${item.name}:${item.size}`" class="flex items-center justify-between gap-2 bg-elevated px-2 py-1">
              <div class="min-w-0">
                <p class="truncate font-mono text-xs text-highlighted">{{ item.name }}</p>
                <p v-if="passwordOf(item)" class="text-xs text-warning">{{ t('linkgrabber.nzb.modal.detected_password') }} <span class="font-mono">{{ passwordOf(item) }}</span></p>
              </div>
              <UButton size="xs" color="neutral" variant="ghost" icon="i-lucide-x" :aria-label="t('linkgrabber.nzb.modal.remove_file')" @click="removeFile(index)" />
            </li>
          </ul>
        </div>
        <UFormField :label="t('linkgrabber.nzb.modal.name')" :description="files.length === 1 ? t('linkgrabber.nzb.modal.name_hint') : t('linkgrabber.nzb.modal.name_single_only')">
          <UInput v-model="name" maxlength="200" class="w-full" :disabled="files.length !== 1" />
        </UFormField>
        <UFormField :label="t('linkgrabber.nzb.modal.category')">
          <SearchableSelect v-model="category" :items="categoryItems" class="w-full" />
        </UFormField>
        <UFormField :label="t('linkgrabber.nzb.modal.priority')">
          <USelect v-model="priority" :items="PRIORITY_ITEMS" value-key="value" class="w-full" />
        </UFormField>
        <UFormField v-if="hasLinkFile" name="import-passphrase" :label="t('linkgrabber.nzb.modal.passphrase')" :description="t('linkgrabber.nzb.modal.passphrase_hint')">
          <UInput v-model="passphrase" type="password" autocomplete="current-password" class="w-full" data-testid="import-passphrase" />
        </UFormField>
        <USwitch v-if="hasContainer" v-model="enqueue" :label="t('linkgrabber.nzb.modal.enqueue')" :description="t('linkgrabber.nzb.modal.enqueue_hint')" data-testid="import-enqueue" />
      </form>
    </template>
    <template #footer>
      <UButton :label="t('common.actions.cancel')" color="neutral" variant="outline" @click="emit('close', null)" />
      <UButton :label="t('linkgrabber.nzb.modal.submit')" icon="i-lucide-file-up" type="submit" form="nzb-import-form" :disabled="!files.length" />
    </template>
  </UModal>
</template>
