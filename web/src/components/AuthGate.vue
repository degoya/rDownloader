<script setup lang="ts">
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { BASE_PATH } from '@/basePath'
import AppSignature from '@/components/AppSignature.vue'
import { translateServerMessage } from '@/i18n/server'
import { useSessionStore } from '@/stores/session'
import { readProviderReturn, withoutProviderReturn } from '@/utils/identityProvider'

const { t } = useI18n()
const session = useSessionStore()

// A sign-in through the identity provider that was refused comes back here with a stable code
// (RD-190-15). Read once and taken out of the address, so a reload does not repeat it.
const providerReturn = readProviderReturn(window.location.search)
const providerError = providerReturn.error
  ? translateServerMessage({ code: providerReturn.error, message: null, params: {} })
  : null
if (providerReturn.error || providerReturn.linked) {
  window.history.replaceState(window.history.state, '', withoutProviderReturn(window.location.href))
}
/** Where a sign-in through the provider comes back to: the page this screen covers. */
function here(): string {
  const path = window.location.pathname.slice(BASE_PATH.length) || '/'
  return `${path}${window.location.search}`
}
const passwordOffered = computed(() => session.setupRequired || session.passwordLogin)
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

function signInWithProvider(): void {
  session.signInWithProvider(here())
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

        <UAlert
          v-if="providerError"
          class="mb-6"
          color="error"
          variant="subtle"
          icon="i-lucide-circle-alert"
          :title="providerError"
          :description="providerReturn.name ? t('auth.provider_account', { name: providerReturn.name }) : undefined"
        />

        <div v-if="session.providerName && !session.setupRequired" class="mb-6">
          <UButton
            block
            size="lg"
            icon="i-lucide-shield-check"
            :label="t('auth.provider', { name: session.providerName })"
            :loading="session.pending"
            @click="signInWithProvider"
          />
          <p
            v-if="session.passkeyOffered || passwordOffered"
            class="mt-3 flex items-center gap-3 text-xs uppercase tracking-wide text-muted"
          >
            <span class="h-px flex-1 bg-muted" />{{ t('auth.or') }}<span class="h-px flex-1 bg-muted" />
          </p>
        </div>

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
          <p v-if="passwordOffered" class="mt-3 flex items-center gap-3 text-xs uppercase tracking-wide text-muted">
            <span class="h-px flex-1 bg-muted" />{{ t('auth.or') }}<span class="h-px flex-1 bg-muted" />
          </p>
        </div>

        <UAlert v-if="session.error" class="mb-4" color="error" variant="subtle" icon="i-lucide-circle-alert" :description="session.error" />
        <p v-if="!passwordOffered" class="text-sm leading-6 text-muted">{{ t('auth.password_off') }}</p>
        <form v-else class="space-y-4" @submit.prevent="submit">
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
            :description="t('auth.code_hint')"
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
