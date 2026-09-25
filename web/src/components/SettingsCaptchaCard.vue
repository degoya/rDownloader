<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { CaptchaConfig, SolverKind, TestCaptchaSolver, UpdateCaptchaConfig } from '@/api/types'
import SectionHeader from '@/components/SectionHeader.vue'

const emit = defineEmits<{ error: [string] }>()
const { t } = useI18n()

/** The endpoint the API falls back to when none is stored. */
const DEFAULT_ENDPOINT = 'https://api.2captcha.com'
/** Matches the range the API clamps to, so a typed value is never silently changed. */
const MIN_TIMEOUT = 15
const MAX_TIMEOUT = 600

const form = reactive<CaptchaConfig>({
  solver: 'none',
  endpoint: '',
  has_api_key: false,
  manual_enabled: true,
  manual_timeout_seconds: 180
})
const apiKey = ref('')
const clearApiKey = ref(false)
const loading = ref(true)
const pending = ref(false)
const testing = ref(false)
const testResult = ref<{ ok: boolean, message: string } | null>(null)
let loadRequest: Promise<boolean> | null = null

const solverItems = computed(() => [
  { label: t('captcha.settings.solver.none'), value: 'none' satisfies SolverKind },
  { label: t('captcha.settings.solver.two_captcha'), value: 'two_captcha_compatible' satisfies SolverKind }
])
const solverActive = computed(() => form.solver !== 'none')
/** Testing needs a key: the typed one, or one already stored. */
const canTest = computed(() => solverActive.value && (apiKey.value.trim().length > 0 || form.has_api_key))

onMounted(() => { loadRequest = load() })

async function load(): Promise<boolean> {
  const response = await api.GET('/api/v1/captcha-config')
  loading.value = false
  if (!response.data) {
    emit('error', responseError(response))
    return false
  }
  apply(response.data)
  return true
}

function apply(config: CaptchaConfig): void {
  Object.assign(form, config)
  apiKey.value = ''
  clearApiKey.value = false
  testResult.value = null
}

/**
 * Asks the service what the key is worth. The key travels in the request body only, and a
 * key typed but not yet saved is tested as-is so a wrong one never has to be stored first.
 */
async function test(): Promise<void> {
  testing.value = true
  testResult.value = null
  const endpoint = form.endpoint.trim()
  const body: TestCaptchaSolver = {
    ...(endpoint ? { endpoint } : {}),
    ...(apiKey.value ? { api_key: apiKey.value } : {})
  }
  const response = await api.POST('/api/v1/captcha-config/test', { body })
  testing.value = false
  testResult.value = response.data
    ? { ok: true, message: t('captcha.settings.test.balance', { balance: response.data.balance }) }
    : { ok: false, message: responseError(response) }
}

async function save(): Promise<boolean> {
  if (loadRequest && !await loadRequest) return false
  pending.value = true
  const endpoint = form.endpoint.trim()
  const body: UpdateCaptchaConfig = {
    solver: form.solver,
    manual_enabled: form.manual_enabled,
    manual_timeout_seconds: form.manual_timeout_seconds,
    // An empty field means "keep whatever is stored" rather than an invalid endpoint.
    ...(endpoint ? { endpoint } : {}),
    ...(apiKey.value ? { api_key: apiKey.value } : {}),
    ...(clearApiKey.value ? { clear_api_key: true } : {})
  }
  const response = await api.PUT('/api/v1/captcha-config', { body })
  pending.value = false
  if (!response.data) {
    emit('error', responseError(response))
    return false
  }
  apply(response.data)
  return true
}

defineExpose({ save })
</script>

<template>
  <section id="captcha-settings" class="border border-muted bg-default p-5">
    <div class="mb-4 flex items-start justify-between gap-4">
      <div>
        <SectionHeader
          :eyebrow="t('captcha.settings.eyebrow')"
          :title="t('captcha.settings.title')"
          :description="t('captcha.settings.description')"
          level="sub"
        />
      </div>
      <UBadge :color="solverActive ? 'success' : 'neutral'" variant="subtle">
        {{ solverActive ? t('captcha.settings.state.active') : t('captcha.settings.state.off') }}
      </UBadge>
    </div>

    <div class="grid gap-3 sm:grid-cols-2">
      <UFormField :label="t('captcha.settings.solver.label')" :description="t('captcha.settings.solver.description')">
        <USelect v-model="form.solver" :items="solverItems" value-key="value" :disabled="loading" class="w-full" />
      </UFormField>
      <UFormField :label="t('captcha.settings.endpoint.label')" :description="t('captcha.settings.endpoint.description')">
        <UInput
          v-model="form.endpoint"
          :placeholder="DEFAULT_ENDPOINT"
          :disabled="loading || !solverActive"
          class="w-full font-mono"
        />
      </UFormField>
      <UFormField
        class="sm:col-span-2"
        :label="t('captcha.settings.api_key.label')"
        :description="form.has_api_key ? t('captcha.settings.api_key.stored') : t('captcha.settings.api_key.description')"
      >
        <UInput
          v-model="apiKey"
          type="password"
          maxlength="512"
          autocomplete="off"
          placeholder="••••••••"
          :disabled="loading || !solverActive || clearApiKey"
          class="w-full font-mono"
        />
      </UFormField>
      <label v-if="form.has_api_key" class="flex items-center gap-3 text-xs text-muted sm:col-span-2">
        <USwitch v-model="clearApiKey" />
        {{ t('captcha.settings.api_key.clear') }}
      </label>
    </div>

    <div class="mt-4 grid gap-3 border-t border-muted pt-4 sm:grid-cols-2">
      <div class="flex items-center justify-between gap-5">
        <div>
          <p class="text-sm font-medium text-highlighted">{{ t('captcha.settings.manual.label') }}</p>
          <p class="mt-1 text-xs leading-5 text-muted">{{ t('captcha.settings.manual.description') }}</p>
        </div>
        <USwitch v-model="form.manual_enabled" :disabled="loading" :aria-label="t('captcha.settings.manual.label')" />
      </div>
      <UFormField :label="t('captcha.settings.timeout.label')" :description="t('captcha.settings.timeout.description')">
        <UInput
          v-model.number="form.manual_timeout_seconds"
          type="number"
          :min="MIN_TIMEOUT"
          :max="MAX_TIMEOUT"
          :disabled="loading || !form.manual_enabled"
          class="w-full"
        >
          <template #trailing><span class="font-mono text-xs text-muted">s</span></template>
        </UInput>
      </UFormField>
    </div>

    <UAlert
      v-if="testResult"
      class="mt-4"
      :color="testResult.ok ? 'success' : 'error'"
      variant="subtle"
      :icon="testResult.ok ? 'i-lucide-circle-check' : 'i-lucide-circle-alert'"
      :title="testResult.message"
    />

    <p class="mt-4 text-xs leading-5 text-muted">{{ t('captcha.settings.hint') }}</p>
    <div class="mt-3 flex justify-end">
      <UButton
        type="button"
        icon="i-lucide-plug-zap"
        color="neutral"
        variant="outline"
        :label="t('captcha.settings.test.button')"
        :loading="testing"
        :disabled="loading || pending || !canTest"
        @click="test"
      />
    </div>
  </section>
</template>
