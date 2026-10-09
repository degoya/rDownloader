<script setup lang="ts">
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { PackageExportChoice, PackageExportFormat } from '@/composables/usePackageExport'

/**
 * Which file an export writes, and the passphrase that seals an `.rdlinks` (RD-1210-01).
 *
 * The passphrase is typed twice, as the settings backup asks for it: a typo would leave a file
 * nobody can open. A crawljob cannot be sealed, so choosing it hides the two fields.
 */
const emit = defineEmits<{ close: [result: PackageExportChoice | null] }>()
const { t } = useI18n()

const MIN_PASSPHRASE = 8

const format = ref<PackageExportFormat>('rdlinks')
const passphrase = ref('')
const confirmation = ref('')

const formats = computed(() => [
  { value: 'rdlinks', label: t('common.export.format_rdlinks'), description: t('common.export.format_rdlinks_hint') },
  { value: 'crawljob', label: t('common.export.format_crawljob'), description: t('common.export.format_crawljob_hint') }
])
const sealed = computed(() => format.value === 'rdlinks' && passphrase.value.length > 0)
const tooShort = computed(() => sealed.value && passphrase.value.length < MIN_PASSPHRASE)
const mismatch = computed(() => sealed.value && confirmation.value.length > 0 && confirmation.value !== passphrase.value)
const ready = computed(() => !sealed.value || (!tooShort.value && confirmation.value === passphrase.value))

function submit(): void {
  if (!ready.value) return
  emit('close', { format: format.value, passphrase: format.value === 'rdlinks' ? passphrase.value : '' })
}
</script>

<template>
  <UModal :title="t('common.export.title')" :description="t('common.export.description')" :close="{ onClick: () => emit('close', null) }">
    <template #body>
      <form id="package-export-form" class="space-y-4" @submit.prevent="submit">
        <URadioGroup v-model="format" :legend="t('common.export.format')" :items="formats" data-testid="package-export-format" />
        <template v-if="format === 'rdlinks'">
          <UFormField name="export-passphrase" :label="t('common.export.passphrase')" :description="t('common.export.passphrase_hint')" :error="tooShort ? t('common.export.passphrase_short') : undefined">
            <UInput v-model="passphrase" type="password" autocomplete="new-password" class="w-full" data-testid="package-export-passphrase" />
          </UFormField>
          <UFormField v-if="passphrase" name="export-confirmation" :label="t('common.export.passphrase_confirm')" :error="mismatch ? t('common.export.passphrase_mismatch') : undefined">
            <UInput v-model="confirmation" type="password" autocomplete="new-password" class="w-full" data-testid="package-export-confirmation" />
          </UFormField>
        </template>
      </form>
    </template>
    <template #footer>
      <UButton :label="t('common.actions.cancel')" color="neutral" variant="outline" @click="emit('close', null)" />
      <UButton :label="t('common.export.submit')" icon="i-lucide-file-down" type="submit" form="package-export-form" :disabled="!ready" data-testid="package-export-submit" />
    </template>
  </UModal>
</template>
