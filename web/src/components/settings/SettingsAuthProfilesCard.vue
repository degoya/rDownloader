<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { AuthMethod, AuthProfile } from '@/api/types'
import FormActions from '@/components/FormActions.vue'
import FormListLayout from '@/components/FormListLayout.vue'
import SectionHeader from '@/components/SectionHeader.vue'
import { useConfirm } from '@/composables/useConfirm'
import { useFormFocus } from '@/composables/useFormFocus'
import { editingRowClass } from '@/utils/editingRow'
import FormFeedback from '@/components/FormFeedback.vue'
import { dateFieldValue, dayOf } from '@/utils/timeFields'
import {
  type AuthProfileForm,
  emptyForm,
  formFor,
  isExpired,
  scopeLabel,
  useAuthProfiles
} from '@/composables/useAuthProfiles'

const { t } = useI18n()
const confirm = useConfirm()

/**
 * The card's own feedback, shown above its form (RD-150-11). It used to travel up to the
 * settings view and appear under the last card of the page, where nobody who pressed a button
 * in this one was looking.
 */
const message = ref<string | null>(null)
const error = ref<string | null>(null)

const {
  profiles, loading, pending, busyId, formError, awaitingApproval,
  refresh, create, update, setEnabled, test, remove
} = useAuthProfiles((event, text) => {
  message.value = event === 'message' ? text : null
  error.value = event === 'error' ? text : null
})
/** A refused save and any other failure share one place; the refused save is the newer one. */
const shownError = computed(() => formError.value ?? error.value)
const formElement = ref<HTMLFormElement | null>(null)
const focusForm = useFormFocus(formElement)

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
  message.value = null
  void focusForm()
}

async function submit(): Promise<void> {
  if (!canSubmit.value) return
  message.value = null
  error.value = null
  const id = editingId.value
  const ok = id ? await update(id, form, clearCertificate.value) : await create(form)
  if (!ok) return
  message.value = t(id ? 'settings.auth_profiles.saved' : 'settings.auth_profiles.created')
  reset()
}

async function confirmRemove(profile: AuthProfile): Promise<void> {
  const confirmed = await confirm({
    title: t('settings.auth_profiles.delete_title'),
    description: t('settings.auth_profiles.delete_description'),
    confirmLabel: t('common.actions.delete'),
    destructive: true
  })
  if (!confirmed) return
  if (editingId.value === profile.id) reset()
  await remove(profile.id)
}
</script>

<template>
  <UCard as="section" data-settings-anchor="accounts.site_logins">
    <UAlert v-if="awaitingApproval.length" class="mb-4" color="warning" :title="t('settings.auth_profiles.approval_title')">
      <template #description>
        <p class="text-xs leading-5 text-muted">{{ t('settings.auth_profiles.approval_description') }}</p>
        <div v-for="profile in awaitingApproval" :key="profile.id" class="mt-3 flex items-center gap-3">
          <UIcon name="i-lucide-globe" class="text-warning" />
          <span class="min-w-0 flex-1 truncate font-mono text-xs text-highlighted">{{ scopeLabel(profile) }}</span>
          <UButton
            size="xs"
            icon="i-lucide-check"
            :label="t('settings.auth_profiles.approve')"
            :loading="busyId === profile.id"
            @click="setEnabled(profile.id, true)"
          />
        </div>
      </template>
    </UAlert>

    <FormListLayout :list-title="t('settings.auth_profiles.title')" :count="profiles.length">
      <template #form>
        <SectionHeader
          class="mb-4"
          :eyebrow="t('settings.auth_profiles.eyebrow')"
          :title="editingId ? t('settings.auth_profiles.form_edit') : t('settings.auth_profiles.form_new')"
          :description="t('settings.auth_profiles.description')"
          level="sub"
        />
        <FormFeedback class="mb-3" :error="shownError" :message="message" testid="auth-profile" />
        <!-- The method decides what the credential is, so it comes first and its two fields
             follow it directly (RD-150-11). -->
        <form ref="formElement" class="grid gap-3" @submit.prevent="submit">
          <UFormField :label="t('settings.auth_profiles.method_label')" required>
            <USelect v-model="form.method" :items="methodItems" class="w-full" />
          </UFormField>
          <UFormField v-if="form.method === 'basic'" :label="t('settings.auth_profiles.username_label')">
            <UInput v-model="form.username" class="w-full" />
          </UFormField>
          <UFormField
            :label="secretLabel"
            :description="form.method === 'cookies' ? t('settings.auth_profiles.secret_cookies_description') : t('settings.auth_profiles.secret_keep')"
            :required="editingId === null"
          >
            <UTextarea v-if="form.method === 'cookies'" v-model="form.secret" :rows="4" class="w-full font-mono text-xs" />
            <UInput v-else v-model="form.secret" type="password" class="w-full" />
          </UFormField>
          <UFormField :label="t('settings.auth_profiles.name_label')" required>
            <UInput v-model="form.name" maxlength="100" :placeholder="t('settings.auth_profiles.name_placeholder')" icon="i-lucide-shield-check" class="w-full" />
          </UFormField>
          <UFormField :label="t('settings.auth_profiles.scope_label')" :description="t('settings.auth_profiles.scope_description')" required>
            <UInput v-model="form.scope" class="w-full font-mono" placeholder="files.example.com/reports" />
          </UFormField>
          <USwitch v-model="form.include_subdomains" :label="t('settings.auth_profiles.subdomains_label')" />
          <UFormField :label="t('settings.auth_profiles.certificate_label')" :description="t('settings.auth_profiles.certificate_description')">
            <UTextarea v-model="form.certificate_pem" :rows="4" class="w-full font-mono text-xs" placeholder="-----BEGIN PRIVATE KEY-----" />
          </UFormField>
          <UCheckbox v-if="editingId" v-model="clearCertificate" :label="t('settings.auth_profiles.certificate_clear')" />
          <UFormField :label="t('settings.auth_profiles.expires_label')" :description="t('settings.auth_profiles.expires_description')">
            <UInputDate :model-value="dateFieldValue(form.expires_at)" class="w-full" @update:model-value="form.expires_at = dayOf($event)" />
          </UFormField>
          <USwitch v-model="form.enabled" :label="t('settings.auth_profiles.enabled_label')" />
          <FormActions
            :editing="editingId !== null"
            :create-label="t('settings.auth_profiles.create_action')"
            create-icon="i-lucide-shield-plus"
            :loading="pending"
            :disabled="!canSubmit"
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
            :class="editingRowClass(editingId === profile.id, 'outline')"
            data-profile-row
          >
            <UAvatar icon="i-lucide-shield-check" color="primary" />
            <div class="min-w-0 flex-1">
              <p class="text-sm font-medium text-highlighted">{{ profile.name }}</p>
              <p class="truncate font-mono text-2xs text-muted">{{ scopeLabel(profile) }}</p>
            </div>
            <UBadge v-if="editingId === profile.id" size="sm" color="primary" variant="subtle">{{ t('common.editing') }}</UBadge>
            <UBadge color="neutral" variant="subtle">{{ profile.method }}</UBadge>
            <UBadge v-if="isExpired(profile)" color="warning" variant="subtle">{{ t('settings.auth_profiles.badge_expired') }}</UBadge>
            <UBadge v-else-if="!profile.enabled" color="neutral" variant="outline">{{ t('settings.auth_profiles.badge_disabled') }}</UBadge>
            <UIcon v-if="profile.has_client_certificate" name="i-lucide-file-badge" class="text-success" :title="t('settings.auth_profiles.badge_certificate')" />
            <UIcon v-if="profile.origin === 'browser_capture'" name="i-lucide-globe" class="text-muted" :title="t('settings.auth_profiles.badge_captured')" />
            <USwitch
              :model-value="profile.enabled"
              :aria-label="t('settings.auth_profiles.enabled_label')"
              :loading="busyId === profile.id"
              @update:model-value="setEnabled(profile.id, $event)"
            />
            <UButton size="xs" color="neutral" variant="ghost" icon="i-lucide-plug-zap" :label="t('common.actions.test')" :loading="busyId === profile.id" @click="test(profile.id)" />
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
              :loading="busyId === profile.id"
              @click="confirmRemove(profile)"
            />
          </div>
          <p v-if="!loading && !profiles.length" class="p-5 text-center text-sm text-muted">{{ t('settings.auth_profiles.empty') }}</p>
        </div>
      </template>
    </FormListLayout>
  </UCard>
</template>
