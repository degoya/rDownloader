<script setup lang="ts">
import { computed, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, resultMessage } from '@/api/client'
import type { CreateStorageRoot, StorageRoot } from '@/api/types'
import DataState from '@/components/DataState.vue'
import FormListLayout from '@/components/FormListLayout.vue'
import { useEditableList } from '@/composables/useEditableList'
import { useFormFocus } from '@/composables/useFormFocus'
import { GIB, byteModel, formatBytes } from '@/utils/format'
import SectionHeader from '@/components/SectionHeader.vue'

const roots = defineModel<StorageRoot[]>({ required: true })
const props = defineProps<{
  /**
   * Path to offer for a new root. The form used to start on a hardcoded `/downloads`, which is
   * right inside the container image and wrong everywhere else — on a native Linux install the
   * filesystem root belongs to another user, so accepting the offer produced a failure. The
   * caller passes the directory the service actually writes to; without one the field starts
   * empty, which is at least honest.
   */
  suggestedPath?: string
  /** True while the tab's fetch is still running; the empty state waits for it (RD-104-07). */
  loading?: boolean | undefined
  /** The tab's fetch failure, so an unreachable service is not drawn as an empty list. */
  loadError?: string | null | undefined
}>()
const { t } = useI18n()
const message = ref<string | null>(null)
const formElement = ref<HTMLFormElement | null>(null)
const focusForm = useFormFocus(formElement)
const deletingId = ref<string | null>(null)
const form = reactive<CreateStorageRoot>({ name: '', path: props.suggestedPath ?? '', is_default: false, minimum_free_bytes: null })

const list = useEditableList<StorageRoot, CreateStorageRoot>({
  list: roots,
  create: body => api.POST('/api/v1/storage-roots', { body }),
  update: (id, body) => api.PUT('/api/v1/storage-roots/{id}', { params: { path: { id } }, body }),
  destroy: id => api.DELETE('/api/v1/storage-roots/{id}', { params: { path: { id } } }),
  reset: () => {
    form.name = ''
    form.path = props.suggestedPath ?? ''
    form.is_default = lockDefault.value
    form.minimum_free_bytes = null
  },
  confirmDelete: root => ({
    title: t('routing.root.delete_title'),
    description: t('routing.root.delete_description', { name: root.name }),
    confirmLabel: t('common.actions.delete'),
    confirmIcon: 'i-lucide-trash-2',
    destructive: true
  })
})
const { editingId, pending, error } = list

// The suggestion is fetched, so it usually arrives after this component is set up. Filling the
// field then is only right while nobody has typed in it and no existing root is being edited —
// overwriting what the user entered would be worse than offering nothing.
watch(
  () => props.suggestedPath,
  (suggested) => {
    if (suggested && !editingId.value && !form.path) form.path = suggested
  }
)

/** Thresholds are entered in GiB; an empty field inherits the global default. */
const minimumFreeGiB = byteModel(
  () => form.minimum_free_bytes,
  (raw) => { form.minimum_free_bytes = raw },
  GIB
)

/** Roots the service says will not survive the container being recreated. */
const ephemeral = computed(() => roots.value.filter(root => root.persistence === 'ephemeral'))

/**
 * The last default cannot be given up -- the service coerces it back, so the switch says so
 * up front instead of letting the user make an edit that quietly does not take.
 */
const lockDefault = computed(() => {
  if (!roots.value.length) return true
  if (!editingId.value) return false
  return roots.value.find(root => root.id === editingId.value)?.is_default === true
})

/** The backend keeps a single default; mirror that locally instead of refetching. */
function applyDefault(list: StorageRoot[], saved: StorageRoot): StorageRoot[] {
  if (!saved.is_default) return list
  return list.map(root => root.id === saved.id ? root : { ...root, is_default: false })
}

async function submit(): Promise<void> {
  message.value = null
  const updating = editingId.value !== null
  const saved = await list.submit({ ...form })
  if (!saved) return
  // A root the server made the default takes it away from whichever root held it before.
  roots.value = applyDefault(roots.value, saved)
  message.value = updating ? t('routing.root.updated') : t('routing.root.created')
}

function edit(root: StorageRoot): void {
  message.value = null
  list.edit(root)
  form.name = root.name
  form.path = root.path
  form.is_default = root.is_default
  form.minimum_free_bytes = root.minimum_free_bytes ?? null
  if (lockDefault.value) form.is_default = true
  void focusForm()
}

async function remove(root: StorageRoot): Promise<void> {
  deletingId.value = root.id
  message.value = null
  const { removed, body } = await list.remove(root)
  deletingId.value = null
  if (removed) message.value = resultMessage(body)
}
</script>

<template>
  <section class="border border-muted bg-default p-5">
    <FormListLayout :list-title="t('routing.root.title')" :count="roots.length">
      <template #form>
        <SectionHeader
          :eyebrow="t('routing.root.eyebrow')"
          :title="editingId ? t('routing.root.form_edit') : t('routing.root.form_new')"
          :description="t('routing.root.description')"
          class="mb-4"
        />
        <UAlert v-if="error" class="mb-3" color="error" variant="subtle" :description="error" />
        <!-- Between the two on purpose: the warning is a standing condition, the success message
             below it a receipt for the last save. Both can be on screen at once. -->
        <UAlert v-if="ephemeral.length" class="mb-3" color="warning" variant="subtle" icon="i-lucide-triangle-alert" :title="t('routing.root.ephemeral_title')" :description="t('routing.root.ephemeral_description')" />
        <UAlert v-if="message" class="mb-3" color="success" variant="subtle" :description="message" />
        <form ref="formElement" class="grid gap-3" @submit.prevent="submit">
          <UFormField :label="t('routing.root.name_label')" :description="t('routing.root.name_description')">
            <UInput v-model="form.name" required maxlength="100" class="w-full" :placeholder="t('routing.root.name_placeholder')" icon="i-lucide-hard-drive" />
          </UFormField>
          <UFormField :label="t('routing.root.path_label')" :description="t('routing.root.path_description')">
            <UInput v-model="form.path" required class="w-full font-mono" :placeholder="t('routing.root.path_placeholder')" icon="i-lucide-folder" />
          </UFormField>
          <UFormField :label="t('routing.root.minimum_free_label')" :description="t('routing.root.minimum_free_description')">
            <UInput v-model.number="minimumFreeGiB" type="number" min="0" step="1" class="w-full" :placeholder="t('routing.root.minimum_free_placeholder')" icon="i-lucide-shield-check">
              <template #trailing><span class="font-mono text-xs text-muted">GiB</span></template>
            </UInput>
          </UFormField>
          <UFormField :label="t('routing.root.default_label')" :description="t('routing.root.default_description')">
            <USwitch v-model="form.is_default" :disabled="lockDefault" :aria-label="t('routing.root.default_label')" />
            <p v-if="lockDefault" class="mt-1 text-[11px] leading-5 text-muted">{{ t('routing.root.default_locked_hint') }}</p>
          </UFormField>
          <div class="flex gap-2">
            <UButton type="submit" :icon="editingId ? 'i-lucide-save' : 'i-lucide-plus'" :label="editingId ? t('common.actions.save') : t('routing.root.create')" :loading="pending" />
            <UButton v-if="editingId" type="button" color="neutral" variant="ghost" icon="i-lucide-x" :label="t('routing.cancel_edit')" @click="list.reset" />
          </div>
        </form>
      </template>
      <template #list>
        <div class="divide-y divide-muted border border-muted">
          <div v-for="root in roots" :key="root.id" class="flex items-center gap-3 p-3" :class="editingId === root.id ? 'border-l-2 border-l-primary' : ''">
            <UIcon name="i-lucide-folder-lock" class="text-primary" />
            <div class="min-w-0 flex-1">
              <p class="text-sm font-medium text-highlighted">{{ root.name }}</p>
              <p class="truncate font-mono text-[11px] text-muted">{{ root.path }}</p>
              <p v-if="root.minimum_free_bytes" class="text-[11px] text-muted">{{ t('routing.root.minimum_free_badge', { value: formatBytes(root.minimum_free_bytes) }) }}</p>
            </div>
            <UBadge v-if="editingId === root.id" color="primary" variant="subtle">{{ t('common.editing') }}</UBadge>
            <UBadge v-if="root.persistence === 'ephemeral'" color="warning" variant="subtle" icon="i-lucide-triangle-alert">{{ t('routing.root.ephemeral_badge') }}</UBadge>
            <UBadge v-if="root.is_default" color="primary" variant="subtle">{{ t('routing.root.default_badge') }}</UBadge>
            <UButton size="xs" color="neutral" variant="ghost" icon="i-lucide-pencil" :aria-label="t('common.actions.edit')" @click="edit(root)" />
            <UButton size="xs" color="error" variant="ghost" icon="i-lucide-trash-2" :aria-label="t('common.actions.delete')" :loading="deletingId === root.id" @click="remove(root)" />
          </div>
          <DataState :loading="props.loading" :error="props.loadError" :empty="!roots.length" variant="inline" class="p-5">
            <p class="text-center text-sm text-muted">{{ t('routing.root.empty') }}</p>
          </DataState>
        </div>
      </template>
    </FormListLayout>
  </section>
</template>
