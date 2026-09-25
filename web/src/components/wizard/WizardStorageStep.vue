<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { CreateCategory, StorageRoot } from '@/api/types'
import RoutingStorageRoots from '@/components/routing/RoutingStorageRoots.vue'

const { t } = useI18n()
const roots = ref<StorageRoot[]>([])
const suggestedPath = ref('')
const categoryError = ref<string | null>(null)

const complete = computed(() => roots.value.length > 0)

onMounted(() => void load())

async function load(): Promise<void> {
  const response = await api.GET('/api/v1/storage-roots')
  if (response.data) roots.value = response.data
  // The service knows the directory it writes to; offering that beats a guess the user has to
  // correct, and beats the container path the form used to start on.
  const status = await api.GET('/api/v1/setup/status')
  if (status.data) suggestedPath.value = status.data.suggested_storage_path
}

/**
 * A storage root alone does not make the queue work: destination resolution needs a default
 * category pointing at one. Create that silently the first time the user leaves this step.
 */
async function ensureDefaultCategory(): Promise<void> {
  categoryError.value = null
  const existing = await api.GET('/api/v1/categories')
  if (!existing.data || existing.data.length > 0) return
  const target = roots.value.find(root => root.is_default) ?? roots.value[0]
  if (!target) return
  const body: CreateCategory = {
    name: t('wizard.storage.default_category_name'),
    color: '#38BDF8',
    storage_root_id: target.id,
    relative_path: '',
    is_default: true
  }
  const response = await api.POST('/api/v1/categories', { body })
  if (!response.data) categoryError.value = responseError(response)
}

defineExpose({ complete, ensureDefaultCategory })
</script>

<template>
  <div class="space-y-5">
    <p class="max-w-3xl text-sm leading-6 text-muted">{{ t('wizard.storage.intro') }}</p>
    <RoutingStorageRoots v-model="roots" :suggested-path="suggestedPath" />
    <UAlert v-if="categoryError" color="error" variant="subtle" :description="categoryError" />
    <p class="flex items-start gap-2 text-xs leading-5 text-muted">
      <UIcon name="i-lucide-info" class="mt-0.5 size-4 shrink-0" />
      <span>{{ t('wizard.storage.default_category_hint') }}</span>
    </p>
    <UAlert
      v-if="!complete"
      color="warning"
      variant="subtle"
      icon="i-lucide-triangle-alert"
      :description="t('wizard.storage.required_hint')"
    />
  </div>
</template>
