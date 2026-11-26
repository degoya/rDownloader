<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { StorageCapacity, StorageCapacityRoot } from '@/api/types'
import { useDebouncedEventRefresh } from '@/composables/useDebouncedEventRefresh'
import { formatBytes } from '@/utils/format'

const { t } = useI18n()
const capacity = ref<StorageCapacity | null>(null)
const error = ref<string | null>(null)
const resuming = ref<string | null>(null)

async function load(): Promise<void> {
  const response = await api.GET('/api/v1/storage/capacity')
  if (response.data) capacity.value = response.data
}

async function resume(root: StorageCapacityRoot): Promise<void> {
  resuming.value = root.target
  error.value = null
  const response = await api.POST('/api/v1/storage/capacity/{target}/resume', {
    params: { path: { target: root.target } }
  })
  resuming.value = null
  if (!response.data) {
    error.value = responseError(response)
    return
  }
  await load()
}

onMounted(() => void load())
// Announced when a target is blocked and when it is released (RD-1260-04); it was polled every
// five seconds in every open tab (RD-1480-18).
useDebouncedEventRefresh(['storage.capacity'], load)

defineExpose({ reload: load })
</script>

<template>
  <div v-if="capacity?.roots.some(root => root.blocked)" class="flex flex-col gap-2">
    <UAlert v-if="error" color="error" role="alert" icon="i-lucide-circle-alert" :description="error" />
    <UAlert
      v-for="root in capacity.roots.filter(entry => entry.blocked)"
      :key="root.target"
      color="warning"
      icon="i-lucide-hard-drive-download"
      :title="t('downloads.capacity.title', { name: root.name })"
    >
      <template #description>
        <p>
          {{ t('downloads.capacity.description', {
            free: formatBytes(root.shortfall?.free_bytes ?? root.free_bytes),
            required: formatBytes(root.shortfall?.required_bytes ?? root.minimum_free_bytes)
          }) }}
        </p>
        <p v-if="root.shortfall && !root.shortfall.size_known" class="mt-1 text-xs">
          {{ t('downloads.capacity.unknown_size') }}
        </p>
        <p class="mt-1 truncate font-mono text-2xs">{{ root.path }}</p>
      </template>
      <template #actions>
        <UButton
          size="xs"
          color="warning"
          variant="solid"
          icon="i-lucide-play"
          :label="t('downloads.capacity.resume')"
          :loading="resuming === root.target"
          @click="resume(root)"
        />
      </template>
    </UAlert>
    <p v-if="!capacity.auto_resume" class="text-xs text-muted">{{ t('downloads.capacity.manual_hint') }}</p>
  </div>
</template>
