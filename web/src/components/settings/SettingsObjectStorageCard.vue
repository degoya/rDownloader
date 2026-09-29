<script setup lang="ts">
/**
 * Object storage profiles (RD-150-04, RD-150-05): S3 and compatible services, Azure Blob Storage
 * and Google Cloud Storage. The form beside the list it feeds, with a live test per profile; the
 * provider comes first and decides the other fields. Profiles save themselves; nothing here
 * belongs to the settings document.
 */
import { computed, onMounted, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import FormActions from '@/components/FormActions.vue'

import type { ObjectStorageProfile } from '@/api/types'
import FormListLayout from '@/components/FormListLayout.vue'
import SectionHeader from '@/components/SectionHeader.vue'
import { useConfirm } from '@/composables/useConfirm'
import { useFormFocus } from '@/composables/useFormFocus'
import {
  PROVIDERS,
  type ObjectStorageForm,
  bucketLink,
  credentialSources,
  emptyForm,
  endpointLabel,
  formComplete,
  formFor,
  isIncomplete,
  keepsSecret,
  storesSecret,
  useObjectStorageProfiles
} from '@/composables/useObjectStorageProfiles'

const { t } = useI18n()
const confirm = useConfirm()
const formElement = ref<HTMLFormElement | null>(null)
const focusForm = useFormFocus(formElement)

const {
  profiles, loading, pending, busyId, error, message,
  clearFeedback, refresh, create, update, remove, test
} = useObjectStorageProfiles()

const form = reactive<ObjectStorageForm>(emptyForm())
const editingId = ref<string | null>(null)
const editing = computed(() => profiles.value.find(profile => profile.id === editingId.value) ?? null)
const isS3 = computed(() => form.provider === 's3')
const isAzure = computed(() => form.provider === 'azure')
const signsWithSecret = computed(() => storesSecret(form.credential_source))
const canSubmit = computed(() => formComplete(form, editing.value))

const providerItems = computed(() =>
  PROVIDERS.map(value => ({ value, label: t(`remote.object_storage.providers.${value}`) }))
)
const sourceItems = computed(() =>
  credentialSources(form.provider).map(value => ({ value, label: t(`remote.object_storage.sources.${value}`) }))
)
/** S3 keeps the hints it always had; the other two have their own per source. */
const sourceHint = computed(() => {
  const group = { s3: 'source_hints', azure: 'azure_hints', gcs: 'gcs_hints' }[form.provider]
  return t(`remote.object_storage.${group}.${form.credential_source}`)
})
/** What the one secret field holds for the provider and the source. */
const secretLabel = computed(() => {
  if (form.provider === 'gcs') return t('remote.object_storage.service_account_key')
  if (form.provider === 'azure') {
    return t(form.credential_source === 'shared_access_signature' ? 'remote.object_storage.sas' : 'remote.object_storage.account_key')
  }
  return t('remote.object_storage.secret_access_key')
})
const endpointHint = computed(() => ({
  s3: t('remote.object_storage.endpoint_description'),
  azure: t('remote.object_storage.endpoint_description_azure'),
  gcs: t('remote.object_storage.endpoint_description_gcs')
})[form.provider])
const endpointPlaceholder = computed(() => ({
  s3: 'https://minio.example:9000',
  azure: 'http://127.0.0.1:10000/devstoreaccount1',
  gcs: 'https://storage.googleapis.com'
})[form.provider])

// A source the new provider cannot sign with falls back to stored keys rather than being sent.
watch(() => form.provider, provider => {
  if (!credentialSources(provider).includes(form.credential_source)) form.credential_source = 'static'
})
const addressingItems = computed(() =>
  (['auto', 'path', 'virtual_host'] as const).map(value => ({
    value,
    label: t(`remote.object_storage.addressing_modes.${value}`)
  }))
)

/** Examples of the link and target forms, passed as parameters so no message holds an `@`. */
const examples = {
  link: 's3://bucket/key, az://container/blob, gs://bucket/object',
  named: 's3://profile@bucket/key',
  target: 'object-storage:<profile-id>/<prefix>'
}

onMounted(() => void refresh())

function reset(): void {
  Object.assign(form, emptyForm())
  editingId.value = null
}

function startEdit(profile: ObjectStorageProfile): void {
  clearFeedback()
  Object.assign(form, formFor(profile))
  editingId.value = profile.id
  void focusForm()
}

async function submit(): Promise<void> {
  if (!canSubmit.value) return
  const id = editingId.value
  // A failed save keeps the form filled; the error stands above it.
  const saved = id ? await update(id, form) : await create(form)
  if (!saved) return
  reset()
  message.value = t(id ? 'remote.object_storage.saved' : 'remote.object_storage.created')
}

async function runTest(profile: ObjectStorageProfile): Promise<void> {
  const outcome = await test(profile.id)
  if (!outcome) return
  if (outcome.ok) message.value = t('remote.object_storage.test_ok', { name: profile.name })
  else error.value = t(`server.codes.${outcome.code}`, outcome.params)
}

async function confirmRemove(profile: ObjectStorageProfile): Promise<void> {
  const confirmed = await confirm({
    title: t('remote.object_storage.delete_title'),
    description: t('remote.object_storage.delete_description', { name: profile.name }),
    confirmLabel: t('common.actions.delete'),
    confirmIcon: 'i-lucide-trash-2',
    destructive: true
  })
  if (!confirmed) return
  // The form must not stay pointed at a profile the server no longer has.
  if (await remove(profile.id) && editingId.value === profile.id) reset()
}
</script>

<template>
  <section data-settings-anchor="transfers.object_storage" class="border border-muted bg-default p-5" data-testid="object-storage-card">
    <SectionHeader
      :eyebrow="t('remote.object_storage.eyebrow')"
      :title="t('remote.object_storage.title')"
      :description="t('remote.object_storage.description')"
      level="sub"
    />
    <p class="mt-2 max-w-3xl text-xs leading-5 text-muted">{{ t('remote.object_storage.usage_links', examples) }}</p>
    <p class="mt-1 mb-4 max-w-3xl text-xs leading-5 text-muted">{{ t('remote.object_storage.usage_upload', examples) }}</p>

    <FormListLayout :list-title="t('remote.object_storage.list_title')" :count="profiles.length">
      <template #form>
        <SectionHeader
          class="mb-3"
          :eyebrow="t('remote.object_storage.list_title')"
          :title="editingId ? t('remote.object_storage.form_edit') : t('remote.object_storage.form_new')"
          level="sub"
        />
        <UAlert v-if="error" class="mb-3" color="error" variant="subtle" :description="error" data-testid="object-storage-error" />
        <UAlert v-if="message" class="mb-3" color="success" variant="subtle" :description="message" data-testid="object-storage-message" />

        <form ref="formElement" class="grid gap-3" @submit.prevent="submit">
          <UFormField :label="t('remote.object_storage.provider')">
            <USelect v-model="form.provider" :items="providerItems" value-key="value" class="w-full" data-testid="object-storage-provider" />
          </UFormField>
          <UFormField :label="t('remote.object_storage.credential_source')" :description="sourceHint">
            <USelect v-model="form.credential_source" :items="sourceItems" value-key="value" class="w-full" data-testid="object-storage-source" />
          </UFormField>
          <UFormField :label="t('remote.object_storage.name')">
            <UInput v-model="form.name" required maxlength="100" :placeholder="t('remote.object_storage.name_placeholder')" icon="i-lucide-cylinder" class="w-full" />
          </UFormField>
          <UFormField v-if="isAzure" :label="t('remote.object_storage.account')" :description="t('remote.object_storage.account_description')">
            <UInput v-model="form.account" required maxlength="24" autocomplete="off" class="w-full font-mono" />
          </UFormField>
          <UFormField :label="t('remote.object_storage.endpoint')" :description="endpointHint">
            <UInput v-model="form.endpoint" type="url" :placeholder="endpointPlaceholder" icon="i-lucide-globe" class="w-full font-mono" />
          </UFormField>
          <UFormField v-if="isS3" :label="t('remote.object_storage.region')" :description="t('remote.object_storage.region_description')">
            <UInput v-model="form.region" placeholder="eu-central-1" class="w-full font-mono" />
          </UFormField>
          <UFormField
            :label="t(isAzure ? 'remote.object_storage.container' : 'remote.object_storage.bucket')"
            :description="t(isAzure ? 'remote.object_storage.container_description' : 'remote.object_storage.bucket_description')"
          >
            <UInput v-model="form.bucket" class="w-full font-mono" />
          </UFormField>
          <UFormField v-if="isS3" :label="t('remote.object_storage.addressing')" :description="t('remote.object_storage.addressing_description')">
            <USelect v-model="form.addressing" :items="addressingItems" value-key="value" class="w-full" />
          </UFormField>

          <UFormField v-if="isS3 && form.credential_source === 'static'" :label="t('remote.object_storage.access_key_id')">
            <UInput v-model="form.access_key_id" autocomplete="off" icon="i-lucide-key-round" class="w-full font-mono" />
          </UFormField>
          <UFormField
            v-if="signsWithSecret"
            :label="secretLabel"
            :description="keepsSecret(form, editing) ? t('remote.object_storage.secret_keep') : t('remote.object_storage.secrets_note')"
          >
            <UInput v-model="form.secret_access_key" type="password" autocomplete="new-password" class="w-full font-mono" />
          </UFormField>
          <template v-if="isS3 && form.credential_source === 'static'">
            <UFormField
              :label="t('remote.object_storage.session_token')"
              :description="editing?.has_session_token ? t('remote.object_storage.session_token_keep') : t('remote.object_storage.session_token_description')"
            >
              <UInput v-model="form.session_token" type="password" autocomplete="new-password" class="w-full font-mono" />
            </UFormField>
            <UCheckbox
              v-if="editing?.has_session_token"
              v-model="form.clear_session_token"
              :label="t('remote.object_storage.clear_session_token')"
            />
          </template>

          <USwitch
            v-if="isS3"
            v-model="form.checksums"
            :label="t('remote.object_storage.checksums')"
            :description="t('remote.object_storage.checksums_description')"
          />
          <USwitch v-model="form.enabled" :label="t('remote.object_storage.enabled')" />

          <FormActions
            :editing="Boolean(editingId)"
            :create-label="t('remote.object_storage.create')"
            :disabled="!canSubmit"
            :loading="pending"
            @cancel="reset"
          />
        </form>
      </template>

      <template #list>
        <div class="divide-y divide-muted border border-muted">
          <div
            v-for="profile in profiles"
            :key="profile.id"
            class="flex flex-wrap items-center gap-3 p-3"
            :class="editingId === profile.id ? 'border-l-2 border-l-primary' : ''"
            data-testid="object-storage-row"
          >
            <span class="grid size-8 place-items-center bg-elevated text-primary">
              <UIcon name="i-lucide-cylinder" />
            </span>
            <div class="min-w-0 flex-1">
              <p class="text-sm font-medium text-highlighted">{{ profile.name }}</p>
              <p class="truncate font-mono text-[11px] text-muted">
                {{ endpointLabel(profile) }} · {{ bucketLink(profile) ?? t('remote.object_storage.any_bucket') }}
              </p>
            </div>
            <UBadge v-if="editingId === profile.id" color="primary" variant="subtle">{{ t('common.editing') }}</UBadge>
            <UBadge color="neutral" variant="outline">{{ t(`remote.object_storage.sources.${profile.credential_source}`) }}</UBadge>
            <UBadge v-if="isIncomplete(profile)" color="warning" variant="subtle">{{ t('remote.object_storage.incomplete') }}</UBadge>
            <UBadge v-if="profile.enabled" color="success" variant="subtle">{{ t('remote.object_storage.enabled') }}</UBadge>
            <UBadge v-else color="neutral" variant="outline">{{ t('remote.object_storage.disabled') }}</UBadge>
            <UButton
              size="xs"
              color="neutral"
              variant="ghost"
              icon="i-lucide-plug-zap"
              :label="t('remote.object_storage.test')"
              :loading="busyId === profile.id"
              @click="runTest(profile)"
            />
            <UButton
              size="xs"
              color="neutral"
              variant="ghost"
              icon="i-lucide-pencil"
              :aria-label="t('common.actions.edit')"
              :title="t('common.actions.edit')"
              @click="startEdit(profile)"
            />
            <UButton
              size="xs"
              color="error"
              variant="ghost"
              icon="i-lucide-trash-2"
              :aria-label="t('common.actions.delete')"
              :title="t('common.actions.delete')"
              :disabled="busyId === profile.id"
              @click="confirmRemove(profile)"
            />
          </div>
          <p v-if="!loading && !profiles.length" class="p-5 text-center text-sm text-muted">{{ t('remote.object_storage.empty') }}</p>
        </div>
      </template>
    </FormListLayout>
  </section>
</template>
