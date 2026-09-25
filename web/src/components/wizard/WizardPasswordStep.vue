<script setup lang="ts">
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { useSessionStore } from '@/stores/session'

const emit = defineEmits<{ done: [] }>()
const { t } = useI18n()
const session = useSessionStore()

const password = ref('')
const confirmation = ref('')

const mismatch = computed(() => confirmation.value.length > 0 && confirmation.value !== password.value)
const canSubmit = computed(() => password.value.length >= 10 && confirmation.value === password.value)

async function submit(): Promise<void> {
  if (!canSubmit.value) return
  if (await session.submitPassword(password.value)) emit('done')
}
</script>

<template>
  <!-- Setup already ran: the backend rejects a second /auth/setup, so only report the state. -->
  <div v-if="!session.setupRequired" class="flex items-start gap-3 border border-success/40 bg-success/10 p-4">
    <UIcon name="i-lucide-shield-check" class="mt-0.5 size-5 shrink-0 text-success" />
    <div>
      <p class="text-sm font-medium text-highlighted">{{ t('wizard.password.configured') }}</p>
      <p class="mt-1 text-sm leading-6 text-muted">{{ t('wizard.password.configured_hint') }}</p>
    </div>
  </div>

  <form v-else class="max-w-md space-y-4" @submit.prevent="submit">
    <UFormField :label="t('auth.password')" :hint="t('wizard.password.rule')" required>
      <UInput
        v-model="password"
        type="password"
        autocomplete="new-password"
        icon="i-lucide-key-round"
        size="lg"
        autofocus
        class="w-full"
      />
    </UFormField>
    <UFormField :label="t('auth.confirm_password')" :error="mismatch ? t('auth.mismatch') : undefined" required>
      <UInput
        v-model="confirmation"
        type="password"
        autocomplete="new-password"
        icon="i-lucide-shield-check"
        size="lg"
        class="w-full"
      />
    </UFormField>
    <UAlert v-if="session.error" color="error" variant="subtle" icon="i-lucide-circle-alert" :description="session.error" />
    <UButton
      type="submit"
      size="lg"
      icon="i-lucide-arrow-right"
      trailing
      :label="t('wizard.password.submit')"
      :loading="session.pending"
      :disabled="!canSubmit"
    />
  </form>
</template>
