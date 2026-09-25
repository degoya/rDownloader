<script setup lang="ts">
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import AppSignature from '@/components/AppSignature.vue'
import { useSessionStore } from '@/stores/session'

const { t } = useI18n()
const session = useSessionStore()
// The listen port is configurable, so it has to come from the actual connection.
const endpoint = window.location.host
const password = ref('')
const confirmation = ref('')
const code = ref('')

const mismatch = computed(() => session.setupRequired
  && confirmation.value.length > 0
  && confirmation.value !== password.value)
const canSubmit = computed(() => password.value.length >= 10
  && (!session.setupRequired || confirmation.value === password.value))

async function submit(): Promise<void> {
  if (!canSubmit.value) return
  await session.submitPassword(password.value, code.value)
}

async function signInWithPasskey(): Promise<void> {
  await session.submitPasskey()
}
</script>

<template>
  <main class="signal-grid grid min-h-screen place-items-center p-5">
    <section class="w-full max-w-md border border-muted bg-default/95 shadow-2xl shadow-primary/5">
      <div class="h-1 transfer-stripe" />
      <div class="p-7 sm:p-9">
        <div class="mb-8 flex items-center gap-3">
          <img src="/favicon.svg" alt="rDownloader" class="size-11" />
          <div>
            <p class="eyebrow">{{ t('auth.endpoint', { endpoint }) }}</p>
            <h1 class="text-xl font-semibold tracking-tight text-highlighted">rDownloader</h1>
          </div>
        </div>

        <p class="mb-6 text-sm leading-6 text-toned">
          {{ session.setupRequired ? t('auth.setup_intro') : t('auth.login_intro') }}
        </p>

        <UAlert
          v-if="session.expired"
          class="mb-6"
          color="warning"
          variant="subtle"
          icon="i-lucide-clock-alert"
          :title="t('auth.expired_title')"
          :description="t('auth.expired_description')"
        />

        <div v-if="session.passkeyOffered && !session.setupRequired" class="mb-6">
          <UButton
            block
            size="lg"
            color="neutral"
            variant="subtle"
            icon="i-lucide-key-round"
            :label="t('auth.passkey')"
            :loading="session.pending"
            @click="signInWithPasskey"
          />
          <p class="mt-3 flex items-center gap-3 text-xs uppercase tracking-wide text-muted">
            <span class="h-px flex-1 bg-muted" />{{ t('auth.or') }}<span class="h-px flex-1 bg-muted" />
          </p>
        </div>

        <form class="space-y-4" @submit.prevent="submit">
          <UFormField :label="t('auth.password')" required>
            <UInput
              v-model="password"
              type="password"
              autocomplete="current-password"
              icon="i-lucide-key-round"
              size="lg"
              autofocus
              class="w-full"
            />
          </UFormField>
          <UFormField v-if="session.setupRequired" :label="t('auth.confirm_password')" :error="mismatch ? t('auth.mismatch') : undefined" required>
            <UInput
              v-model="confirmation"
              type="password"
              autocomplete="new-password"
              icon="i-lucide-shield-check"
              size="lg"
              class="w-full"
            />
          </UFormField>
          <UFormField
            v-if="session.mfaRequired"
            :label="t('auth.code')"
            :help="t('auth.code_hint')"
            required
          >
            <UInput
              v-model="code"
              inputmode="text"
              autocomplete="one-time-code"
              icon="i-lucide-shield-check"
              size="lg"
              autofocus
              class="w-full"
            />
          </UFormField>
          <UAlert v-if="session.error" color="error" variant="subtle" icon="i-lucide-circle-alert" :description="session.error" />
          <UButton
            type="submit"
            block
            size="lg"
            icon="i-lucide-log-in"
            :label="session.setupRequired ? t('auth.setup') : t('auth.login')"
            :loading="session.pending"
            :disabled="!canSubmit"
          />
        </form>
      </div>
      <AppSignature />
    </section>
  </main>
</template>
