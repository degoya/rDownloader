<script setup lang="ts">
import { ref } from 'vue'
import { useI18n } from 'vue-i18n'

const emit = defineEmits<{
  close: [result: { text: string, packageName: string | null, password: string | null } | null]
}>()
const { t } = useI18n()
const text = ref('')
const packageName = ref('')
const password = ref('')

function submit(): void {
  if (!text.value.trim()) return
  emit('close', {
    text: text.value,
    packageName: packageName.value.trim() || null,
    password: password.value.trim() || null
  })
}
</script>

<template>
  <UModal :title="t('linkgrabber.intake.title')" :description="t('linkgrabber.intake.description')" :close="{ onClick: () => emit('close', null) }" :ui="{ footer: 'justify-end', content: 'sm:max-w-2xl' }">
    <template #body>
      <form id="collector-intake-form" class="space-y-3" @submit.prevent="submit">
        <UTextarea v-model="text" :rows="10" autoresize autofocus :placeholder="t('linkgrabber.intake.placeholder')" class="w-full font-mono text-xs" />
        <div class="grid gap-3 sm:grid-cols-2">
          <UFormField :label="t('linkgrabber.intake.package_name')" :description="t('linkgrabber.intake.package_name_hint')">
            <UInput v-model="packageName" maxlength="200" class="w-full" />
          </UFormField>
          <UFormField :label="t('linkgrabber.intake.password')">
            <UInput v-model="password" type="password" maxlength="1024" class="w-full font-mono" />
          </UFormField>
        </div>
      </form>
    </template>
    <template #footer>
      <UButton :label="t('common.actions.cancel')" color="neutral" variant="outline" @click="emit('close', null)" />
      <UButton :label="t('linkgrabber.intake.submit')" icon="i-lucide-scan-search" type="submit" form="collector-intake-form" :disabled="!text.trim()" />
    </template>
  </UModal>
</template>
