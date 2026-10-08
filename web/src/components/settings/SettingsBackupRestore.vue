<script setup lang="ts">
/**
 * *Backup & restore* (RD-1160-01): the settings file, exported and imported here, on
 * *Configuration*; the scheduled encrypted full backup with its destinations on *Full backup*;
 * restoring one on *Restore*.
 */
import { computed, ref } from 'vue'
import { useToast } from '@nuxt/ui/composables'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { SettingsBundle } from '@/api/types'
import { useConfirm } from '@/composables/useConfirm'
import { JsonRefusal, useJsonImport } from '@/composables/useJsonImport'
import { subTabItems } from '@/composables/useSettingsSubTab'
import SectionHeader from '@/components/SectionHeader.vue'
import SettingsFullBackupCard from '@/components/settings/SettingsFullBackupCard.vue'
import SettingsFullRestoreCard from '@/components/settings/SettingsFullRestoreCard.vue'
import { useSessionStore } from '@/stores/session'
import { downloadJson } from '@/utils/jsonFile'
import { isRecord } from '@/utils/values'

const emit = defineEmits<{ imported: [] }>()
/** Owned by the settings view, which keeps it in the address. */
const activeTab = defineModel<string>('subTab', { default: 'config' })
const { t } = useI18n()
const tabItems = computed(() => subTabItems('backup', t))
const toast = useToast()
const confirm = useConfirm()
const session = useSessionStore()
const includeSecrets = ref(true)
const exportPassphrase = ref('')
const exportConfirmation = ref('')
const exporting = ref(false)
const exportError = ref<string | null>(null)
const selectedFileName = ref<string | null>(null)
const importBundle = ref<SettingsBundle | null>(null)
const importPassphrase = ref('')
/** The administrator password, typed again: the import replaces the way in (RD-1190-19). */
const importPassword = ref('')
const importing = ref(false)
const importError = ref<string | null>(null)

const exportReady = computed(() => !includeSecrets.value || (
  exportPassphrase.value.length >= 8
  && exportPassphrase.value === exportConfirmation.value
))
const importNeedsPassphrase = computed(() => importBundle.value?.secrets != null)
// With the login switched off there is no password to ask for.
const importNeedsPassword = computed(() => !session.loginDisabled)

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
  downloadJson(response.data, 'settings')
  exportPassphrase.value = ''
  exportConfirmation.value = ''
  toast.add({
    title: t('system.backup.export.success'),
    color: 'success',
    icon: 'i-lucide-file-check-2'
  })
}

/** Chosen and checked here, restored only from the form below — a passphrase may still be due. */
const jsonImport = useJsonImport<SettingsBundle>({
  check: (parsed) => {
    if (!isRecord(parsed) || parsed.format !== 'rdownloader-settings-bundle') {
      return new JsonRefusal(t('system.backup.import.invalid_file'))
    }
    if (parsed.version !== 1) {
      return new JsonRefusal(t('system.backup.import.unsupported_version', { version: String(parsed.version) }))
    }
    return isSettingsBundle(parsed) ? parsed : new JsonRefusal(t('system.backup.import.invalid_file'))
  },
  unreadable: () => t('system.backup.import.invalid_file'),
  refuse: (message) => { importError.value = message },
  take: (bundle, file) => {
    selectedFileName.value = file.name
    importBundle.value = bundle
  }
})
async function selectFile(file: File | null | undefined): Promise<void> {
  importError.value = null
  importBundle.value = null
  selectedFileName.value = null
  importPassphrase.value = ''
  importPassword.value = ''
  await jsonImport.select(file)
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
      passphrase: importNeedsPassphrase.value ? importPassphrase.value : null,
      password: importNeedsPassword.value ? importPassword.value : null
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
    + response.data.indexers
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
  importPassword.value = ''
}

function isSettingsBundle(value: Record<string, unknown>): value is SettingsBundle {
  return value.format === 'rdownloader-settings-bundle'
    && value.version === 1
    && typeof value.exported_at === 'string'
    && typeof value.app_version === 'string'
    && isRecord(value.settings)
}
</script>

<template>
  <div class="space-y-6">
    <header>
      <SectionHeader
        :eyebrow="t('settings.headers.backup.eyebrow')"
        :title="t('settings.headers.backup.title')"
        :description="t('settings.headers.backup.description')"
        level="page"
      />
    </header>

    <UTabs
      v-model="activeTab"
      :items="tabItems"
      :unmount-on-hide="false"
      variant="pill"
      class="w-full"
    >
      <template #config>
        <div class="grid gap-4 xl:grid-cols-2">
          <UCard as="section" data-settings-anchor="backup.export">
            <SectionHeader
              :eyebrow="t('system.backup.export.eyebrow')"
              :title="t('system.backup.export.title')"
              :description="t('system.backup.export.description')"
            />
            <UAlert v-if="exportError" class="mt-5" color="error" icon="i-lucide-circle-alert" :description="exportError" />
            <form class="mt-5 space-y-4" @submit.prevent="downloadBackup">
              <UFormField
                name="include-secrets"
                :label="t('system.backup.export.include_secrets')"
                :description="t('system.backup.export.include_secrets_description')"
                orientation="horizontal"
              >
                <USwitch v-model="includeSecrets" />
              </UFormField>
              <template v-if="includeSecrets">
                <UFormField data-settings-anchor="backup.export_passphrase" name="export-passphrase" :label="t('system.backup.export.passphrase')" required>
                  <UInput v-model="exportPassphrase" type="password" autocomplete="new-password" required class="w-full" />
                </UFormField>
                <UFormField name="export-confirmation" :label="t('system.backup.export.confirm_passphrase')" required>
                  <UInput v-model="exportConfirmation" type="password" autocomplete="new-password" required class="w-full" />
                </UFormField>
              </template>
              <UButton
                type="submit"
                icon="i-lucide-download"
                :label="t('system.backup.export.button')"
                :disabled="!exportReady"
                :loading="exporting"
              />
            </form>
          </UCard>

          <UCard as="section" data-settings-anchor="backup.import">
            <SectionHeader
              :eyebrow="t('system.backup.import.eyebrow')"
              :title="t('system.backup.import.title')"
              :description="t('system.backup.import.description')"
            />
            <UAlert v-if="importError" class="mt-5" color="error" icon="i-lucide-circle-alert" :description="importError" />
            <form class="mt-5 space-y-4" @submit.prevent="restoreBackup">
              <div class="flex flex-wrap items-center gap-3">
                <UFileUpload v-slot="{ open }" :model-value="null" accept=".json" reset :dropzone="false" @update:model-value="selectFile">
                  <UButton
                    type="button"
                    icon="i-lucide-file-json-2"
                    :label="t('system.backup.import.choose_file')"
                    color="neutral"
                    variant="outline"
                    @click="open()"
                  />
                </UFileUpload>
                <span v-if="selectedFileName" class="min-w-0 truncate text-sm text-toned">{{ selectedFileName }}</span>
              </div>
              <UAlert
                v-if="importBundle"
                color="neutral"
                :icon="importNeedsPassphrase ? 'i-lucide-lock-keyhole' : 'i-lucide-lock-keyhole-open'"
                :ui="{ icon: 'size-4 text-primary', description: 'flex flex-wrap items-center gap-2 text-xs text-toned' }"
              >
                <template #description>
                  <span>{{ importNeedsPassphrase ? t('system.backup.import.encrypted') : t('system.backup.import.without_secrets') }}</span>
                  <span class="ml-auto font-mono text-muted">v{{ importBundle.version }} · rDownloader {{ importBundle.app_version }}</span>
                </template>
              </UAlert>
              <UFormField
                v-if="importNeedsPassphrase"
                name="import-passphrase"
                :label="t('system.backup.import.passphrase')"
              >
                <UInput v-model="importPassphrase" type="password" autocomplete="current-password" class="w-full" />
              </UFormField>
              <UFormField
                v-if="importBundle && importNeedsPassword"
                name="import-password"
                :label="t('system.backup.import.password')"
                :description="t('system.backup.import.password_hint')"
                required
              >
                <UInput v-model="importPassword" type="password" autocomplete="current-password" class="w-full" />
              </UFormField>
              <UButton
                type="submit"
                icon="i-lucide-database-backup"
                :label="t('system.backup.import.button')"
                color="error"
                variant="soft"
                :disabled="!importBundle || (importNeedsPassphrase && !importPassphrase) || (importNeedsPassword && !importPassword)"
                :loading="importing"
              />
            </form>
          </UCard>
        </div>
      </template>
      <template #full>
        <SettingsFullBackupCard />
      </template>
      <template #restore>
        <SettingsFullRestoreCard />
      </template>
    </UTabs>
  </div>
</template>
