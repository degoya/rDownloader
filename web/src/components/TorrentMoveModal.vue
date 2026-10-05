<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api } from '@/api/client'
import type { StorageRoot } from '@/api/types'
import { useTorrentsStore } from '@/stores/torrents'

/**
 * "Change location" of a torrent (RD-1100-10): a storage root and a folder below it, the same two
 * fields a category's destination is made of. The package keeps a folder of its own name there.
 * A refusal — the folder is taken, a copy would not fit — stays in the dialog; a started move
 * closes it with the server's message.
 */
const props = defineProps<{ downloadId: string }>()
const emit = defineEmits<{ close: [message: string | null] }>()
const { t } = useI18n()
const torrents = useTorrentsStore()

const roots = ref<StorageRoot[]>([])
const rootId = ref('')
const relativePath = ref('')
const busy = ref(false)
const error = ref<string | null>(null)
const rootItems = computed(() => roots.value.map(root => ({ label: `${root.name} — ${root.path}`, value: root.id })))

onMounted(async () => {
  const response = await api.GET('/api/v1/storage-roots')
  roots.value = response.data ?? []
  rootId.value = (roots.value.find(root => root.is_default) ?? roots.value[0])?.id ?? ''
})

async function submit(): Promise<void> {
  if (!rootId.value || busy.value) return
  busy.value = true
  error.value = null
  const outcome = await torrents.move(props.downloadId, {
    storage_root_id: rootId.value,
    relative_path: relativePath.value.trim()
  })
  busy.value = false
  if (outcome.error) {
    error.value = outcome.error
    return
  }
  emit('close', outcome.message)
}
</script>

<template>
  <UModal :title="t('torrent.move.title')" :description="t('torrent.move.description')" :close="{ onClick: () => emit('close', null) }" :ui="{ footer: 'justify-end' }">
    <template #body>
      <form id="torrent-move-form" class="grid gap-3" @submit.prevent="submit">
        <UAlert v-if="!roots.length" color="warning" variant="subtle" icon="i-lucide-hard-drive" :description="t('torrent.move.no_roots')" />
        <UFormField :label="t('torrent.move.root')">
          <USelect v-model="rootId" :items="rootItems" value-key="value" icon="i-lucide-hard-drive" class="w-full" />
        </UFormField>
        <UFormField :label="t('torrent.move.path')">
          <UInput v-model="relativePath" :placeholder="t('torrent.move.path_placeholder')" icon="i-lucide-corner-down-right" class="w-full font-mono" />
        </UFormField>
        <UAlert v-if="error" color="error" variant="subtle" icon="i-lucide-circle-alert" :description="error" />
      </form>
    </template>
    <template #footer>
      <UButton :label="t('common.actions.cancel')" color="neutral" variant="outline" @click="emit('close', null)" />
      <UButton :label="t('torrent.move.submit')" icon="i-lucide-folder-input" type="submit" form="torrent-move-form" :loading="busy" :disabled="!rootId" />
    </template>
  </UModal>
</template>
