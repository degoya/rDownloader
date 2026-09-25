<script setup lang="ts">
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

const props = defineProps<{
  /** Names of the files the reset applies to, in queue order. */
  files: string[]
  /** Whether the selection contains at least one finished download. */
  hasCompleted: boolean
}>()

const emit = defineEmits<{
  close: [result: { confirmed: boolean, deleteFiles: boolean }]
}>()

const { t } = useI18n()
/** Deliberately off: the finished file is often the only copy that exists. */
const deleteFiles = ref(false)
const SHOWN = 8
const shown = computed(() => props.files.slice(0, SHOWN))
const overflow = computed(() => Math.max(0, props.files.length - SHOWN))
</script>

<template>
  <UModal
    :title="t('downloads.reset.title', { count: props.files.length }, props.files.length)"
    :description="t('downloads.reset.description')"
    :close="{ onClick: () => emit('close', { confirmed: false, deleteFiles: false }) }"
    :ui="{ footer: 'justify-end' }"
  >
    <template #body>
      <ul class="max-h-48 space-y-0.5 overflow-y-auto text-sm text-toned">
        <li v-for="name in shown" :key="name" class="truncate" :title="name">{{ name }}</li>
        <li v-if="overflow" class="text-muted">{{ t('downloads.reset.more_files', { count: overflow }, overflow) }}</li>
      </ul>
      <UCheckbox
        v-if="props.hasCompleted"
        v-model="deleteFiles"
        class="mt-4"
        color="error"
        :label="t('downloads.reset.delete_files')"
        :description="t('downloads.reset.delete_files_hint')"
      />
    </template>
    <template #footer>
      <UButton
        :label="t('common.actions.cancel')"
        color="neutral"
        variant="outline"
        @click="emit('close', { confirmed: false, deleteFiles: false })"
      />
      <UButton
        :label="t('downloads.reset.confirm')"
        icon="i-lucide-rotate-ccw"
        color="error"
        @click="emit('close', { confirmed: true, deleteFiles })"
      />
    </template>
  </UModal>
</template>
