<script setup lang="ts">
import { computed, ref } from 'vue'
import { useToast } from '@nuxt/ui/composables'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { SettingsBundle } from '@/api/types'
import { useConfirm } from '@/composables/useConfirm'
import SectionHeader from '@/components/SectionHeader.vue'

const emit = defineEmits<{ imported: [] }>()
const { t } = useI18n()
const toast = useToast()
const confirm = useConfirm()
const includeSecrets = ref(true)
const exportPassphrase = ref('')
const exportConfirmation = ref('')
const exporting = ref(false)
const exportError = ref<string | null>(null)
const fileInput = ref<HTMLInputElement | null>(null)
const selectedFileName = ref<string | null>(null)
const importBundle = ref<SettingsBundle | null>(null)
const importPassphrase = ref('')
const importing = ref(false)
const importError = ref<string | null>(null)

const exportReady = computed(() => !includeSecrets.value || (
  exportPassphrase.value.length >= 8
  && exportPassphrase.value === exportConfirmation.value
))
const importNeedsPassphrase = computed(() => importBundle.value?.secrets != null)

async function downloadBackup(): Promise<void> {
  exportError.value = null
  if (includeSecrets.value && exportPassphrase.value.length < 8) {
    exportError.value = t('system.backup.export.passphrase_short')
    return
  }
  if (includeSecrets.value && exportPassphrase.value !== exportConfirmation.value) {
    exportError.value = t('system.backup.export.passphrase_mismatch')
    return
  }
  exporting.value = true
  const response = await api.POST('/api/v1/settings/export', {
    body: {
      include_secrets: includeSecrets.value,
      passphrase: includeSecrets.value ? exportPassphrase.value : null
    }
  })
  exporting.value = false
  if (!response.data) {
    exportError.value = responseError(response)
    return
  }
  const blob = new Blob([JSON.stringify(response.data, null, 2)], { type: 'application/json' })
  const url = URL.createObjectURL(blob)
  const anchor = document.createElement('a')
  anchor.href = url
  anchor.download = `rdownloader-settings-${new Date().toISOString().slice(0, 10)}.json`
  document.body.append(anchor)
  anchor.click()
  anchor.remove()
  URL.revokeObjectURL(url)
  exportPassphrase.value = ''
  exportConfirmation.value = ''
  toast.add({
    title: t('system.backup.export.success'),
    color: 'success',
    icon: 'i-lucide-file-check-2'
  })
}

function chooseFile(): void {
  if (!fileInput.value) return
  fileInput.value.value = ''
  fileInput.value.click()
}

async function selectFile(event: Event): Promise<void> {
  importError.value = null
  importBundle.value = null
  selectedFileName.value = null
  importPassphrase.value = ''
  const target = event.target
  const file = target instanceof HTMLInputElement ? target.files?.item(0) : null
  if (!file) return
  try {
    const parsed: unknown = JSON.parse(await file.text())
    if (!isRecord(parsed) || parsed.format !== 'rdownloader-settings-bundle') {
      importError.value = t('system.backup.import.invalid_file')
      return
    }
    if (parsed.version !== 1) {
      importError.value = t('system.backup.import.unsupported_version', { version: String(parsed.version) })
      return
    }
    if (!isSettingsBundle(parsed)) {
      importError.value = t('system.backup.import.invalid_file')
      return
    }
    selectedFileName.value = file.name
    importBundle.value = parsed
  } catch {
    importError.value = t('system.backup.import.invalid_file')
  }
}

async function restoreBackup(): Promise<void> {
  if (!importBundle.value) return
  importError.value = null
  if (importNeedsPassphrase.value && !importPassphrase.value) {
    importError.value = t('system.backup.import.passphrase_required')
    return
  }
  const accepted = await confirm({
    title: t('system.backup.import.confirm_title'),
    description: t('system.backup.import.confirm_description'),
    confirmLabel: t('system.backup.import.confirm'),
    confirmIcon: 'i-lucide-database-backup',
    destructive: true
  })
  if (!accepted) return
  importing.value = true
  const response = await api.POST('/api/v1/settings/import', {
    body: {
      bundle: importBundle.value,
      passphrase: importNeedsPassphrase.value ? importPassphrase.value : null
    }
  })
  importing.value = false
  if (!response.data) {
    importError.value = responseError(response)
    return
  }
  const count = response.data.storage_roots
    + response.data.categories
    + response.data.category_rules
    + response.data.hotfolders
    + response.data.stream_channels
    + response.data.proxy_profiles
    + response.data.accounts
    + response.data.usenet_servers
  toast.add({
    title: t('system.backup.import.success'),
    description: t('system.backup.import.success_description', { count }),
    color: 'success',
    icon: 'i-lucide-database-backup'
  })
  emit('imported')
  importBundle.value = null
  selectedFileName.value = null
  importPassphrase.value = ''
}

function isSettingsBundle(value: Record<string, unknown>): value is SettingsBundle {
  return value.format === 'rdownloader-settings-bundle'
    && value.version === 1
    && typeof value.exported_at === 'string'
    && typeof value.app_version === 'string'
    && isRecord(value.settings)
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null
}
</script>

<template>
  <div class="grid gap-4 xl:grid-cols-2">
    <header class="mb-2 xl:col-span-2">
      <SectionHeader
        :eyebrow="t('settings.headers.backup.eyebrow')"
        :title="t('settings.headers.backup.title')"
        :description="t('settings.headers.backup.description')"
        level="page"
      />
    </header>
    <section class="border border-muted bg-default p-5">
      <SectionHeader
        :eyebrow="t('system.backup.export.eyebrow')"
        :title="t('system.backup.export.title')"
        :description="t('system.backup.export.description')"
      />
      <form class="mt-5 space-y-4" @submit.prevent="downloadBackup">
        <UFormField
          name="include-secrets"
          :label="t('system.backup.export.include_secrets')"
          :description="t('system.backup.export.include_secrets_description')"
        >
          <USwitch v-model="includeSecrets" />
        </UFormField>
        <div v-if="includeSecrets" class="grid gap-3 sm:grid-cols-2">
          <UFormField name="export-passphrase" :label="t('system.backup.export.passphrase')">
            <UInput v-model="exportPassphrase" type="password" autocomplete="new-password" class="w-full" />
          </UFormField>
          <UFormField name="export-confirmation" :label="t('system.backup.export.confirm_passphrase')">
            <UInput v-model="exportConfirmation" type="password" autocomplete="new-password" class="w-full" />
          </UFormField>
        </div>
        <UAlert v-if="exportError" color="error" variant="subtle" icon="i-lucide-circle-alert" :description="exportError" />
        <UButton
          type="submit"
          icon="i-lucide-download"
          :label="t('system.backup.export.button')"
          :disabled="!exportReady"
          :loading="exporting"
        />
      </form>
    </section>

    <section class="border border-muted bg-default p-5">
      <SectionHeader
        :eyebrow="t('system.backup.import.eyebrow')"
        :title="t('system.backup.import.title')"
        :description="t('system.backup.import.description')"
      />
      <div class="mt-5 space-y-4">
        <input ref="fileInput" class="hidden" type="file" accept=".json,application/json" @change="selectFile">
        <div class="flex flex-wrap items-center gap-3">
          <UButton
            type="button"
            icon="i-lucide-file-json-2"
            :label="t('system.backup.import.choose_file')"
            color="neutral"
            variant="outline"
            @click="chooseFile"
          />
          <span v-if="selectedFileName" class="min-w-0 truncate text-sm text-toned">{{ selectedFileName }}</span>
        </div>
        <div v-if="importBundle" class="flex items-center gap-2 border border-muted bg-elevated p-3 text-xs text-toned">
          <UIcon :name="importNeedsPassphrase ? 'i-lucide-lock-keyhole' : 'i-lucide-lock-keyhole-open'" class="size-4 text-primary" />
          <span>{{ importNeedsPassphrase ? t('system.backup.import.encrypted') : t('system.backup.import.without_secrets') }}</span>
          <span class="ml-auto font-mono text-muted">v{{ importBundle.version }} · rDownloader {{ importBundle.app_version }}</span>
        </div>
        <UFormField
          v-if="importNeedsPassphrase"
          name="import-passphrase"
          :label="t('system.backup.import.passphrase')"
        >
          <UInput v-model="importPassphrase" type="password" autocomplete="current-password" class="w-full" />
        </UFormField>
        <UAlert v-if="importError" color="error" variant="subtle" icon="i-lucide-circle-alert" :description="importError" />
        <UButton
          type="button"
          icon="i-lucide-database-backup"
          :label="t('system.backup.import.button')"
          color="error"
          variant="soft"
          :disabled="!importBundle || (importNeedsPassphrase && !importPassphrase)"
          :loading="importing"
          @click="restoreBackup"
        />
      </div>
    </section>
  </div>
</template>
