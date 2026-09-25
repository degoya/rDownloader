<script setup lang="ts">
import { ref } from 'vue'
import { useI18n } from 'vue-i18n'

const props = withDefaults(defineProps<{
  title: string
  label: string
  value: string
  description?: string
  maxLength?: number
}>(), {
  description: '',
  maxLength: 255
})
const emit = defineEmits<{
  close: [value: string | null]
}>()
const { t } = useI18n()
const draft = ref(props.value)

function submit(): void {
  const trimmed = draft.value.trim()
  if (!trimmed || trimmed === props.value) return emit('close', null)
  emit('close', trimmed)
}
</script>

<template>
  <UModal :title="title" :description="description" :close="{ onClick: () => emit('close', null) }" :ui="{ footer: 'justify-end' }">
    <template #body>
      <form id="rename-form" @submit.prevent="submit">
        <UFormField :label="label">
          <UInput v-model="draft" :maxlength="maxLength" autofocus class="w-full font-mono" />
        </UFormField>
      </form>
    </template>
    <template #footer>
      <UButton :label="t('common.actions.cancel')" color="neutral" variant="outline" @click="emit('close', null)" />
      <UButton :label="t('common.actions.rename')" icon="i-lucide-pencil-line" type="submit" form="rename-form" :disabled="!draft.trim() || draft.trim() === value" />
    </template>
  </UModal>
</template>
