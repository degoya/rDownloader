<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useToast } from '@nuxt/ui/composables'
import { useI18n } from 'vue-i18n'
import { renderSVG } from 'uqr'

import { api, responseError } from '@/api/client'
import type { components } from '@/api/schema'
import { useConfirm } from '@/composables/useConfirm'
import { formatMoment } from '@/utils/format'
import SectionHeader from '@/components/SectionHeader.vue'

type MfaStatus = components['schemas']['MfaStatus']
type MfaCredential = components['schemas']['MfaCredential']

const { t } = useI18n()
const status = ref<MfaStatus | null>(null)
const error = ref<string | null>(null)
const busy = ref(false)

/// The enrolment in progress. Held here because the secret and the recovery codes are shown
/// exactly once — reloading the page loses them, which is why the card says so out loud.
const pending = ref<{
  credentialId: string
  provisioningUri: string
  secret: string
  recoveryCodes: string[]
} | null>(null)
const confirmCode = ref('')
const confirmError = ref<string | null>(null)
const disablePassword = ref('')
const freshCodes = ref<string[] | null>(null)

const confirm = useConfirm()
const toast = useToast()

const enabled = computed(() => status.value?.enabled ?? false)
/// Passkeys live in the same table but are a different feature, and have their own card.
const credentials = computed(() => (status.value?.credentials ?? []).filter((entry) => entry.kind === 'totp'))
/// Running out of recovery codes only becomes visible at the worst possible moment, so the
/// card says something before that.
const lowOnCodes = computed(() => enabled.value && (status.value?.recovery_codes_remaining ?? 0) <= 2)

/// The enrolment address as a scannable code.
///
/// Rendered here rather than fetched: the server already hands us the `otpauth://` address, and
/// asking it for a picture of the same thing would put the shared secret on the wire twice.
///
/// Black on white whatever the page theme is. A scanner expects the contrast the QR
/// specification assumes, and an inverted code is not reliably readable — so the frame around
/// the image below stays white in dark mode too, instead of the code being flipped with it.
const qrCode = computed(() => {
  const uri = pending.value?.provisioningUri
  if (!uri) return null
  const svg = renderSVG(uri, { border: 2, pixelSize: 6, whiteColor: '#ffffff', blackColor: '#000000' })
  return `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`
})

onMounted(() => { void load() })

async function load(): Promise<void> {
  const response = await api.GET('/api/v1/mfa')
  if (response.data) {
    status.value = response.data
    error.value = null
  } else {
    error.value = responseError(response)
  }
}

async function startEnrolment(): Promise<void> {
  busy.value = true
  const response = await api.POST('/api/v1/mfa/totp', { body: { label: null } })
  busy.value = false
  if (!response.data) {
    error.value = responseError(response)
    return
  }
  pending.value = {
    credentialId: response.data.credential_id,
    provisioningUri: response.data.provisioning_uri,
    secret: response.data.secret,
    recoveryCodes: response.data.recovery_codes
  }
  confirmCode.value = ''
  confirmError.value = null
  await load()
}

async function confirmEnrolment(): Promise<void> {
  if (!pending.value) return
  busy.value = true
  confirmError.value = null
  const response = await api.POST('/api/v1/mfa/totp/{id}/confirm', {
    params: { path: { id: pending.value.credentialId } },
    body: { code: confirmCode.value }
  })
  busy.value = false
  if (!response.data) {
    confirmError.value = responseError(response)
    return
  }
  pending.value = null
  await load()
  toast.add({ title: t('system.mfa.enabled_toast'), color: 'success', icon: 'i-lucide-shield-check' })
}

async function removeCredential(credential: MfaCredential): Promise<void> {
  const confirmed = await confirm({
    title: t('system.mfa.remove.title'),
    description: t('system.mfa.remove.description', { label: credential.label }),
    confirmLabel: t('system.mfa.remove.confirm'),
    confirmIcon: 'i-lucide-shield-off',
    destructive: true
  })
  if (!confirmed) return
  const response = await api.DELETE('/api/v1/mfa/credentials/{id}', {
    params: { path: { id: credential.id } }
  })
  if (!response.data) {
    error.value = responseError(response)
    return
  }
  await load()
}

async function disable(): Promise<void> {
  busy.value = true
  error.value = null
  const response = await api.POST('/api/v1/mfa/disable', {
    body: { password: disablePassword.value }
  })
  busy.value = false
  if (!response.data) {
    error.value = responseError(response)
    return
  }
  disablePassword.value = ''
  await load()
  toast.add({ title: t('system.mfa.disabled_toast'), color: 'success', icon: 'i-lucide-shield-off' })
}

async function regenerate(): Promise<void> {
  const confirmed = await confirm({
    title: t('system.mfa.regenerate.title'),
    description: t('system.mfa.regenerate.description'),
    confirmLabel: t('system.mfa.regenerate.confirm'),
    confirmIcon: 'i-lucide-refresh-cw',
    destructive: true
  })
  if (!confirmed) return
  const response = await api.POST('/api/v1/mfa/recovery-codes')
  if (!response.data) {
    error.value = responseError(response)
    return
  }
  freshCodes.value = response.data
  await load()
}

async function copy(value: string): Promise<void> {
  await navigator.clipboard.writeText(value)
  toast.add({ title: t('system.mfa.copied'), color: 'success', icon: 'i-lucide-copy-check' })
}

</script>

<template>
  <section class="border border-muted bg-default p-5">
    <SectionHeader :eyebrow="t('system.mfa.eyebrow')" :title="t('system.mfa.title')" :description="t('system.mfa.description')" />

    <UAlert v-if="error" class="mt-3" color="error" variant="subtle" :description="error" />

    <!-- Enrolment in progress: the one time the secret and the codes are visible. -->
    <div v-if="pending" class="mt-4 border border-warning/40 bg-warning/10 p-4">
      <p class="text-sm font-medium text-warning">{{ t('system.mfa.enrol.once') }}</p>
      <p class="mt-3 text-sm text-toned">{{ t('system.mfa.enrol.scan') }}</p>
      <img
        v-if="qrCode"
        class="mt-3 w-44 max-w-full border border-muted bg-white p-2"
        :src="qrCode"
        :alt="t('system.mfa.enrol.qr_alt')"
        width="176"
        height="176"
      >
      <p class="mt-3 text-sm text-toned">{{ t('system.mfa.enrol.manual') }}</p>
      <div class="mt-2 flex flex-wrap items-center gap-2">
        <code class="min-w-0 flex-1 break-all font-mono text-xs leading-5 text-highlighted">{{ pending.secret }}</code>
        <UButton size="xs" color="neutral" variant="soft" icon="i-lucide-copy" :label="t('system.mfa.enrol.copy_secret')" @click="copy(pending.secret)" />
        <UButton size="xs" color="neutral" variant="soft" icon="i-lucide-link" :label="t('system.mfa.enrol.copy_uri')" @click="copy(pending.provisioningUri)" />
      </div>

      <p class="mt-4 text-sm text-toned">{{ t('system.mfa.enrol.recovery') }}</p>
      <ul class="mt-2 grid grid-cols-2 gap-1 font-mono text-xs text-highlighted sm:grid-cols-3">
        <li v-for="entry in pending.recoveryCodes" :key="entry">{{ entry }}</li>
      </ul>
      <UButton
        class="mt-2"
        size="xs"
        color="neutral"
        variant="soft"
        icon="i-lucide-copy"
        :label="t('system.mfa.enrol.copy_codes')"
        @click="copy(pending.recoveryCodes.join('\n'))"
      />

      <form class="mt-4 flex flex-wrap items-end gap-2" @submit.prevent="confirmEnrolment">
        <UFormField class="flex-1" :label="t('system.mfa.enrol.confirm_label')" :error="confirmError ?? undefined">
          <UInput v-model="confirmCode" inputmode="numeric" maxlength="10" class="w-full" />
        </UFormField>
        <UButton type="submit" icon="i-lucide-shield-check" :label="t('system.mfa.enrol.confirm')" :loading="busy" />
      </form>
    </div>

    <!-- Not enrolled and nothing in progress. -->
    <div v-else-if="!enabled" class="mt-4">
      <UButton icon="i-lucide-shield-plus" :label="t('system.mfa.enrol.start')" :loading="busy" @click="startEnrolment" />
    </div>

    <!-- Enrolled. -->
    <template v-if="credentials.length > 0">
      <ul class="mt-4 divide-y divide-muted border border-muted">
        <li v-for="credential in credentials" :key="credential.id" class="flex flex-wrap items-center justify-between gap-3 p-3">
          <div>
            <p class="flex items-center gap-2 text-sm font-medium text-highlighted">
              {{ credential.label }}
              <UBadge v-if="!credential.confirmed_at" color="warning" variant="subtle" size="sm">
                {{ t('system.mfa.unconfirmed') }}
              </UBadge>
            </p>
            <p class="mt-1 text-xs text-muted">{{ t('system.mfa.last_used', { moment: formatMoment(credential.last_used_at) || t('system.mfa.never_used') }) }}</p>
          </div>
          <UButton size="sm" color="neutral" variant="ghost" icon="i-lucide-shield-off" :label="t('system.mfa.remove.action')" @click="removeCredential(credential)" />
        </li>
      </ul>
    </template>

    <div v-if="enabled" class="mt-4 space-y-4">
      <UAlert
        :color="lowOnCodes ? 'warning' : 'neutral'"
        variant="subtle"
        :icon="lowOnCodes ? 'i-lucide-triangle-alert' : 'i-lucide-life-buoy'"
        :description="t('system.mfa.remaining', { count: status?.recovery_codes_remaining ?? 0 })"
      />
      <UButton size="sm" color="neutral" variant="soft" icon="i-lucide-refresh-cw" :label="t('system.mfa.regenerate.action')" @click="regenerate" />

      <div v-if="freshCodes" class="border border-warning/40 bg-warning/10 p-3">
        <p class="text-sm font-medium text-warning">{{ t('system.mfa.enrol.once') }}</p>
        <ul class="mt-2 grid grid-cols-2 gap-1 font-mono text-xs text-highlighted sm:grid-cols-3">
          <li v-for="entry in freshCodes" :key="entry">{{ entry }}</li>
        </ul>
      </div>

      <form class="flex flex-wrap items-end gap-2 border-t border-muted pt-4" @submit.prevent="disable">
        <UFormField class="flex-1" :label="t('system.mfa.disable.label')" :help="t('system.mfa.disable.hint')">
          <UInput v-model="disablePassword" type="password" autocomplete="current-password" class="w-full" />
        </UFormField>
        <UButton type="submit" color="neutral" variant="soft" icon="i-lucide-shield-off" :label="t('system.mfa.disable.action')" :loading="busy" :disabled="!disablePassword" />
      </form>
    </div>
  </section>
</template>
