<script setup lang="ts">
/**
 * Changing the administrator password (RD-120-22).
 *
 * The screen that did not exist: until this card there was no supported way to replace the
 * password at all, so one that had ended up in a note or a screenshot could only be removed
 * by editing the SQLite file by hand.
 *
 * Two things are said out loud rather than left to be discovered. Every session ends, and
 * this window is the one that stays — otherwise the change looks like it signed you out and
 * the natural reaction is to assume it failed. And API tokens keep working, because they do
 * not hang off the password; anyone who knew the old one could have minted some, so the card
 * points at the token list instead of pretending the question does not exist.
 *
 * The confirmation field is checked here and nowhere else: a typo in the replacement is a
 * mistake only the person typing can see, and the server has no second copy to compare.
 */
import { computed, ref } from 'vue'
import { useToast } from '@nuxt/ui/composables'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import SectionHeader from '@/components/SectionHeader.vue'

const { t } = useI18n()
const toast = useToast()

const currentPassword = ref('')
const newPassword = ref('')
const confirmPassword = ref('')
const error = ref<string | null>(null)
const busy = ref(false)

const mismatch = computed(
  () => confirmPassword.value.length > 0 && confirmPassword.value !== newPassword.value
)
const submittable = computed(
  () => currentPassword.value.length > 0 && newPassword.value.length > 0 && !mismatch.value
)

async function submit(): Promise<void> {
  if (!submittable.value) return
  busy.value = true
  error.value = null
  const response = await api.POST('/api/v1/auth/password', {
    body: { current_password: currentPassword.value, new_password: newPassword.value }
  })
  busy.value = false
  if (!response.data) {
    error.value = responseError(response)
    return
  }
  // Cleared whatever happens next: leaving three filled password fields on screen after a
  // successful change is exactly the habit this card exists to break.
  currentPassword.value = ''
  newPassword.value = ''
  confirmPassword.value = ''
  toast.add({ title: t('system.password.changed_toast'), color: 'success', icon: 'i-lucide-key-round' })
}
</script>

<template>
  <section class="border border-muted bg-default p-5">
    <SectionHeader
      :eyebrow="t('system.password.eyebrow')"
      :title="t('system.password.title')"
      :description="t('system.password.description')"
    />

    <UAlert v-if="error" class="mt-3" color="error" variant="subtle" :description="error" />

    <form class="mt-4 space-y-4" @submit.prevent="submit">
      <UFormField :label="t('system.password.current_label')">
        <UInput v-model="currentPassword" type="password" autocomplete="current-password" class="w-full" />
      </UFormField>
      <UFormField :label="t('system.password.new_label')" :help="t('system.password.new_hint')">
        <UInput v-model="newPassword" type="password" autocomplete="new-password" class="w-full" />
      </UFormField>
      <UFormField
        :label="t('system.password.confirm_label')"
        :error="mismatch ? t('system.password.mismatch') : undefined"
      >
        <UInput v-model="confirmPassword" type="password" autocomplete="new-password" class="w-full" />
      </UFormField>
      <UButton
        type="submit"
        color="neutral"
        variant="soft"
        icon="i-lucide-key-round"
        :label="t('system.password.action')"
        :loading="busy"
        :disabled="!submittable"
      />
    </form>

    <div class="mt-4 space-y-2 border-t border-muted pt-4">
      <p class="text-xs text-muted">{{ t('system.password.sessions_hint') }}</p>
      <p class="text-xs text-muted">{{ t('system.password.tokens_hint') }}</p>
    </div>
  </section>
</template>
