<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { AuthMethod, AuthProfile } from '@/api/types'
import SectionHeader from '@/components/SectionHeader.vue'
import { useConfirm } from '@/composables/useConfirm'
import {
  type AuthProfileForm,
  emptyForm,
  formFor,
  isExpired,
  scopeLabel,
  useAuthProfiles
} from '@/composables/useAuthProfiles'

const emit = defineEmits<{ message: [string], error: [string] }>()
const { t } = useI18n()
const confirm = useConfirm()

const {
  profiles, loading, pending, busyId, formError, awaitingApproval,
  refresh, create, update, setEnabled, test, remove
} = useAuthProfiles((event, text) => {
  if (event === 'message') emit('message', text)
  else emit('error', text)
})

const form = reactive<AuthProfileForm>(emptyForm())
const editingId = ref<string | null>(null)
const clearCertificate = ref(false)

const methodItems = computed(() => [
  { label: t('settings.auth_profiles.method_cookies'), value: 'cookies' satisfies AuthMethod },
  { label: t('settings.auth_profiles.method_basic'), value: 'basic' satisfies AuthMethod },
  { label: t('settings.auth_profiles.method_bearer'), value: 'bearer' satisfies AuthMethod }
])
const secretLabel = computed(() => t(`settings.auth_profiles.secret_${form.method}`))
/** A new profile always needs a credential; an edit may keep the stored one. */
const canSubmit = computed(() =>
  form.name.trim().length > 0 && form.scope.trim().length > 0
  && (editingId.value !== null || form.secret.trim().length > 0)
)

onMounted(() => void refresh())

function reset(): void {
  Object.assign(form, emptyForm())
  editingId.value = null
  clearCertificate.value = false
  formError.value = null
}

function startEdit(profile: AuthProfile): void {
  Object.assign(form, formFor(profile))
  editingId.value = profile.id
  clearCertificate.value = false
  formError.value = null
}

async function submit(): Promise<void> {
  const id = editingId.value
  const ok = id ? await update(id, form, clearCertificate.value) : await create(form)
  if (!ok) return
  emit('message', t(id ? 'settings.auth_profiles.saved' : 'settings.auth_profiles.created'))
  reset()
}

async function confirmRemove(profile: AuthProfile): Promise<void> {
  const confirmed = await confirm({
    title: t('settings.auth_profiles.delete_title'),
    description: t('settings.auth_profiles.delete_description'),
    confirmLabel: t('settings.auth_profiles.delete'),
    destructive: true
  })
  if (!confirmed) return
  if (editingId.value === profile.id) reset()
  await remove(profile.id)
}
</script>

<template>
  <section class="border border-muted bg-default p-5">
    <div class="mb-4 flex items-start justify-between">
      <div>
        <SectionHeader
          :eyebrow="t('settings.auth_profiles.eyebrow')"
          :title="t('settings.auth_profiles.title')"
          :description="t('settings.auth_profiles.description')"
          level="sub"
        />
      </div>
      <UBadge color="neutral" variant="outline">{{ profiles.length }}</UBadge>
    </div>

    <div v-if="awaitingApproval.length" class="mb-4 border border-warning bg-warning/5 p-4">
      <p class="text-sm font-medium text-highlighted">{{ t('settings.auth_profiles.approval_title') }}</p>
      <p class="mt-1 text-xs leading-5 text-muted">{{ t('settings.auth_profiles.approval_description') }}</p>
      <div v-for="profile in awaitingApproval" :key="profile.id" class="mt-3 flex items-center gap-3">
        <UIcon name="i-lucide-globe" class="text-warning" />
        <span class="min-w-0 flex-1 truncate font-mono text-xs">{{ scopeLabel(profile) }}</span>
        <UButton
          size="xs"
          icon="i-lucide-check"
          :label="t('settings.auth_profiles.approve')"
          :loading="busyId === profile.id"
          @click="setEnabled(profile.id, true)"
        />
      </div>
    </div>

    <div class="grid gap-3 sm:grid-cols-2">
      <UFormField :label="t('settings.auth_profiles.name_label')">
        <UInput v-model="form.name" maxlength="100" :placeholder="t('settings.auth_profiles.name_placeholder')" icon="i-lucide-shield-check" class="w-full" />
      </UFormField>
      <UFormField :label="t('settings.auth_profiles.method_label')">
        <USelect v-model="form.method" :items="methodItems" class="w-full" />
      </UFormField>
      <UFormField class="sm:col-span-2" :label="t('settings.auth_profiles.scope_label')" :description="t('settings.auth_profiles.scope_description')">
        <UInput v-model="form.scope" class="w-full font-mono" placeholder="files.example.com/reports" />
      </UFormField>
      <UCheckbox v-model="form.include_subdomains" :label="t('settings.auth_profiles.subdomains_label')" />
      <UCheckbox v-model="form.enabled" :label="t('settings.auth_profiles.enabled_label')" />
      <UFormField v-if="form.method === 'basic'" :label="t('settings.auth_profiles.username_label')">
        <UInput v-model="form.username" class="w-full" />
      </UFormField>
      <UFormField
        :class="form.method === 'basic' ? '' : 'sm:col-span-2'"
        :label="secretLabel"
        :description="form.method === 'cookies' ? t('settings.auth_profiles.secret_cookies_description') : t('settings.auth_profiles.secret_keep')"
      >
        <UTextarea v-if="form.method === 'cookies'" v-model="form.secret" :rows="4" class="w-full font-mono text-xs" />
        <UInput v-else v-model="form.secret" type="password" class="w-full" />
      </UFormField>
      <UFormField class="sm:col-span-2" :label="t('settings.auth_profiles.certificate_label')" :description="t('settings.auth_profiles.certificate_description')">
        <UTextarea v-model="form.certificate_pem" :rows="4" class="w-full font-mono text-xs" placeholder="-----BEGIN PRIVATE KEY-----" />
      </UFormField>
      <UCheckbox v-if="editingId" v-model="clearCertificate" :label="t('settings.auth_profiles.certificate_clear')" />
      <UFormField :label="t('settings.auth_profiles.expires_label')" :description="t('settings.auth_profiles.expires_description')">
        <UInput v-model="form.expires_at" type="date" class="w-full" />
      </UFormField>
      <div class="flex gap-2 sm:col-span-2">
        <UButton
          type="button"
          icon="i-lucide-plus"
          :label="editingId ? t('settings.auth_profiles.save') : t('settings.auth_profiles.create')"
          :disabled="!canSubmit"
          :loading="pending"
          @click="submit"
        />
        <UButton v-if="editingId" type="button" color="neutral" variant="ghost" :label="t('settings.auth_profiles.cancel')" @click="reset" />
      </div>
      <UAlert
        v-if="formError"
        class="sm:col-span-2"
        color="error"
        variant="subtle"
        icon="i-lucide-circle-alert"
        :description="formError"
      />
    </div>

    <div class="mt-4 divide-y divide-muted border border-muted">
      <div v-for="profile in profiles" :key="profile.id" class="flex flex-wrap items-center gap-3 p-3">
        <span class="grid size-8 place-items-center bg-elevated text-primary"><UIcon name="i-lucide-shield-check" /></span>
        <div class="min-w-0 flex-1">
          <p class="text-sm font-medium text-highlighted">{{ profile.name }}</p>
          <p class="truncate font-mono text-[11px] text-muted">{{ scopeLabel(profile) }}</p>
        </div>
        <UBadge color="neutral" variant="subtle">{{ profile.method }}</UBadge>
        <UBadge v-if="isExpired(profile)" color="warning" variant="subtle">{{ t('settings.auth_profiles.badge_expired') }}</UBadge>
        <UBadge v-else-if="!profile.enabled" color="neutral" variant="outline">{{ t('settings.auth_profiles.badge_disabled') }}</UBadge>
        <UIcon v-if="profile.has_client_certificate" name="i-lucide-file-badge" class="text-success" :title="t('settings.auth_profiles.badge_certificate')" />
        <UIcon v-if="profile.origin === 'browser_capture'" name="i-lucide-globe" class="text-muted" :title="t('settings.auth_profiles.badge_captured')" />
        <USwitch :model-value="profile.enabled" :loading="busyId === profile.id" @update:model-value="setEnabled(profile.id, $event)" />
        <UButton size="xs" color="neutral" variant="ghost" icon="i-lucide-plug-zap" :label="t('settings.auth_profiles.test')" :loading="busyId === profile.id" @click="test(profile.id)" />
        <UButton size="xs" color="neutral" variant="ghost" icon="i-lucide-pencil" :label="t('settings.auth_profiles.edit')" @click="startEdit(profile)" />
        <UButton size="xs" color="error" variant="ghost" icon="i-lucide-trash-2" :label="t('settings.auth_profiles.delete')" :loading="busyId === profile.id" @click="confirmRemove(profile)" />
      </div>
      <p v-if="!loading && !profiles.length" class="p-5 text-center text-sm text-muted">{{ t('settings.auth_profiles.empty') }}</p>
    </div>
  </section>
</template>
