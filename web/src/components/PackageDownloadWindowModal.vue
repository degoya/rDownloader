<script setup lang="ts">
/**
 * A download package's own download window (RD-1240-30), opened from the package's menu by
 * `usePackageDownloadWindow`. Switched off, the package follows its category's window, or the
 * bandwidth schedule alone when the category has none; the description says which.
 */
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { DownloadWindow } from '@/api/types'
import DownloadWindowEditor from '@/components/DownloadWindowEditor.vue'
import { describeWindow, draftOf, windowBody } from '@/utils/downloadWindow'

const props = defineProps<{
  /** The package's name, for the dialog's description. */
  name: string
  /** The package's own window, `null` while it follows its category's. */
  current: DownloadWindow | null
  /** The window of the package's category, if it has one. */
  categoryWindow: DownloadWindow | null
  /** The timezone the times are read in. */
  timezone: string
}>()
const emit = defineEmits<{ close: [result: { window: DownloadWindow | null } | null] }>()

const { t } = useI18n()
const draft = ref(draftOf(props.current))
const offDescription = computed(() => props.categoryWindow
  ? t('downloads.window.own_off_category', { window: describeWindow(props.categoryWindow, t) || t('downloads.window.glyph_bypass') })
  : t('downloads.window.own_off_schedule'))

function submit(): void {
  emit('close', { window: windowBody(draft.value) })
}
</script>

<template>
  <UModal
    :title="t('downloads.window.title')"
    :description="t('downloads.window.description', { name: props.name, zone: props.timezone })"
    :close="{ onClick: () => emit('close', null) }"
  >
    <template #body>
      <form id="package-download-window-form" @submit.prevent="submit">
        <DownloadWindowEditor v-model="draft" :label="t('downloads.window.own_label')" :description="offDescription" />
      </form>
    </template>
    <template #footer>
      <UButton :label="t('common.actions.cancel')" color="neutral" variant="outline" @click="emit('close', null)" />
      <UButton :label="t('common.actions.save')" icon="i-lucide-save" type="submit" form="package-download-window-form" data-testid="download-window-save" />
    </template>
  </UModal>
</template>
