<script setup lang="ts">
/**
 * Signing in through an identity provider (RD-190-15, ADR 0021): the provider, the account there
 * that is the administrator, and the password sign-in switch.
 *
 * Every change here creates or removes a way in, so each asks for the password again, as adding
 * a passkey does. The account is linked by signing in at the provider once — never typed — and
 * the password form can only be switched off from a session the provider opened; only
 * `rdownloader auth password-login on` on the machine itself turns it back on.
 */
import { computed, onMounted, reactive, ref } from 'vue'
import { useToast } from '@nuxt/ui/composables'
import { useI18n } from 'vue-i18n'
import { useRoute, useRouter } from 'vue-router'

import { api, responseError } from '@/api/client'
import type { components } from '@/api/schema'
import SectionHeader from '@/components/SectionHeader.vue'
import { useConfirm } from '@/composables/useConfirm'
import { useCopy } from '@/composables/useCopy'
import { translateServerMessage } from '@/i18n/server'
import { formatMoment } from '@/utils/format'
import { leaveFor } from '@/utils/identityProvider'

type OidcSettings = components['schemas']['OidcSettings']

/** The command that turns the password sign-in back on, shown so it can be copied. */
const BREAK_GLASS = 'rdownloader auth password-login on'

const { t } = useI18n()
const toast = useToast()
const confirm = useConfirm()
const copyToClipboard = useCopy()
const route = useRoute()
const router = useRouter()

const settings = ref<OidcSettings | null>(null)
const loading = ref(true)
const busy = ref(false)
const error = ref<string | null>(null)
/** The password every change here asks for again. */
const password = ref('')
const form = reactive({
  issuer: '',
  clientId: '',
  clientSecret: '',
  displayName: '',
  groupClaim: '',
  groupValue: '',
  providerLogout: false
})

const configured = computed(() => settings.value?.configured ?? false)
const linked = computed(() => settings.value?.identity ?? null)
/** Another provider or client gets its own secret; the stored one is kept only for the same. */
const secretNeeded = computed(() => !configured.value
  || form.issuer.trim() !== (settings.value?.issuer ?? '')
  || form.clientId.trim() !== (settings.value?.client_id ?? ''))
const canSave = computed(() => Boolean(
  password.value
  && form.issuer.trim()
  && form.clientId.trim()
  && form.displayName.trim()
  && (!secretNeeded.value || form.clientSecret.trim())
  && Boolean(form.groupClaim.trim()) === Boolean(form.groupValue.trim())
))

onMounted(() => {
  noticeReturn()
  void load()
})

/** A link comes back here from the provider: say how it went, then tidy the address. */
function noticeReturn(): void {
  const code = typeof route.query.oidc_error === 'string' ? route.query.oidc_error : null
  if (route.query.oidc === 'linked') {
    toast.add({ title: t('system.oidc.linked_toast'), color: 'success', icon: 'i-lucide-link' })
  } else if (code) {
    error.value = translateServerMessage({ code, message: null, params: {} })
  } else {
    return
  }
  const query = { ...route.query }
  delete query.oidc
  delete query.oidc_error
  delete query.oidc_name
  void router.replace({ path: route.path, query, hash: route.hash })
}

async function load(): Promise<void> {
  const response = await api.GET('/api/v1/auth/oidc')
  loading.value = false
  if (!response.data) {
    error.value = responseError(response)
    return
  }
  settings.value = response.data
  form.issuer = response.data.issuer ?? ''
  form.clientId = response.data.client_id ?? ''
  form.clientSecret = ''
  form.displayName = response.data.display_name ?? ''
  form.groupClaim = response.data.group_claim ?? ''
  form.groupValue = response.data.group_value ?? ''
  form.providerLogout = response.data.provider_logout
}

async function save(): Promise<void> {
  busy.value = true
  error.value = null
  const response = await api.PUT('/api/v1/auth/oidc', {
    body: {
      password: password.value,
      issuer: form.issuer.trim(),
      client_id: form.clientId.trim(),
      client_secret: form.clientSecret.trim() || null,
      display_name: form.displayName.trim(),
      group_claim: form.groupClaim.trim() || null,
      group_value: form.groupValue.trim() || null,
      provider_logout: form.providerLogout
    }
  })
  busy.value = false
  if (!response.data) {
    error.value = responseError(response)
    return
  }
  password.value = ''
  await load()
  toast.add({ title: t('system.oidc.saved_toast'), color: 'success', icon: 'i-lucide-shield-check' })
}

async function remove(): Promise<void> {
  const confirmed = await confirm({
    title: t('system.oidc.remove.title'),
    description: t('system.oidc.remove.description'),
    confirmLabel: t('system.oidc.remove.confirm'),
    confirmIcon: 'i-lucide-trash-2',
    destructive: true
  })
  if (!confirmed) return
  await act(() => api.DELETE('/api/v1/auth/oidc', { body: { password: password.value } }))
}

/** Links an account at the provider: the page goes there and comes back to this card. */
async function link(): Promise<void> {
  busy.value = true
  error.value = null
  const response = await api.POST('/api/v1/auth/oidc/link', { body: { password: password.value } })
  if (!response.data) {
    busy.value = false
    error.value = responseError(response)
    return
  }
  leaveFor(response.data.authorization_url)
}

async function unlink(): Promise<void> {
  const confirmed = await confirm({
    title: t('system.oidc.unlink.title'),
    description: t('system.oidc.unlink.description'),
    confirmLabel: t('system.oidc.unlink.confirm'),
    confirmIcon: 'i-lucide-unlink',
    destructive: true
  })
  if (!confirmed) return
  await act(() => api.DELETE('/api/v1/auth/oidc/identity', { body: { password: password.value } }))
}

async function switchPasswordOff(): Promise<void> {
  const confirmed = await confirm({
    title: t('system.oidc.password_off.title'),
    description: t('system.oidc.password_off.description', { command: BREAK_GLASS }),
    confirmLabel: t('system.oidc.password_off.confirm'),
    confirmIcon: 'i-lucide-lock',
    destructive: true
  })
  if (!confirmed) return
  await act(() => api.POST('/api/v1/auth/password-login/off', { body: { password: password.value } }))
}

/** One change that answers with a message: run it, then show the card as it is now. */
async function act(request: () => Promise<{ data?: unknown; error?: unknown }>): Promise<void> {
  busy.value = true
  error.value = null
  const response = await request()
  busy.value = false
  if (response.error !== undefined || !response.data) {
    error.value = responseError(response)
    return
  }
  password.value = ''
  await load()
}

async function copy(value: string): Promise<void> {
  if (!(await copyToClipboard(value))) return
  toast.add({ title: t('system.oidc.copied'), color: 'success', icon: 'i-lucide-copy-check' })
}
</script>

<template>
  <section data-settings-anchor="security.oidc" class="border border-muted bg-default p-5">
    <SectionHeader :eyebrow="t('system.oidc.eyebrow')" :title="t('system.oidc.title')" :description="t('system.oidc.description')" />

    <UAlert v-if="error" class="mt-3" color="error" variant="subtle" :description="error" />

    <template v-if="!loading && settings">
      <!-- What to register at the provider. Derived from the external URL, never from a request. -->
      <div class="mt-4">
        <p class="text-xs font-medium uppercase tracking-wide text-muted">{{ t('system.oidc.redirect_uri') }}</p>
        <div v-if="settings.redirect_uri" class="mt-1 flex flex-wrap items-center gap-2">
          <code class="break-all font-mono text-sm text-highlighted">{{ settings.redirect_uri }}</code>
          <UButton
            size="xs"
            color="neutral"
            variant="ghost"
            icon="i-lucide-copy"
            :aria-label="t('system.oidc.copy')"
            :title="t('system.oidc.copy')"
            @click="copy(settings.redirect_uri)"
          />
        </div>
        <UAlert
          v-else
          class="mt-2"
          color="warning"
          variant="subtle"
          icon="i-lucide-triangle-alert"
          :description="t('system.oidc.external_url_missing')"
        />
      </div>

      <!-- The account at the provider that is the administrator. -->
      <div v-if="configured" class="mt-4 flex flex-wrap items-center justify-between gap-3 border border-muted p-3">
        <div>
          <p class="text-sm font-medium text-highlighted">
            {{ linked ? t('system.oidc.linked', { label: linked.label || t('system.oidc.unnamed') }) : t('system.oidc.not_linked') }}
          </p>
          <p class="mt-1 text-xs text-muted">
            {{ linked ? t('system.oidc.linked_at', { moment: formatMoment(linked.linked_at) }) : t('system.oidc.link_hint') }}
          </p>
        </div>
        <UButton
          v-if="linked"
          size="sm"
          color="error"
          variant="soft"
          icon="i-lucide-unlink"
          :label="t('system.oidc.unlink.action')"
          :loading="busy"
          :disabled="!password || !settings.password_login"
          @click="unlink"
        />
        <UButton
          v-else
          size="sm"
          icon="i-lucide-link"
          :label="t('system.oidc.link')"
          :loading="busy"
          :disabled="!password || !settings.redirect_uri"
          @click="link"
        />
      </div>

      <!-- The password form: off only from a provider session, back on only from the machine. -->
      <div v-if="linked" class="mt-4">
        <UAlert
          v-if="!settings.password_login"
          color="info"
          variant="subtle"
          icon="i-lucide-lock"
          :title="t('system.oidc.password_is_off')"
          :description="t('system.oidc.password_is_off_hint', { command: BREAK_GLASS })"
          :actions="[{ label: t('system.oidc.copy_command'), icon: 'i-lucide-copy', color: 'neutral', variant: 'outline', onClick: () => copy(BREAK_GLASS) }]"
        />
        <div v-else class="flex flex-wrap items-center justify-between gap-3">
          <p class="text-xs text-muted">
            {{ settings.provider_session ? t('system.oidc.password_off.ready') : t('system.oidc.password_off.needs_provider_session') }}
          </p>
          <UButton
            size="sm"
            color="warning"
            variant="soft"
            icon="i-lucide-lock"
            :label="t('system.oidc.password_off.action')"
            :loading="busy"
            :disabled="!password || !settings.provider_session"
            @click="switchPasswordOff"
          />
        </div>
      </div>

      <form class="mt-4 grid gap-4 sm:grid-cols-2" @submit.prevent="save">
        <UFormField class="sm:col-span-2" :label="t('system.oidc.issuer')" :description="t('system.oidc.issuer_hint')" required>
          <UInput v-model="form.issuer" type="url" placeholder="https://auth.example.com/application/o/rdownloader/" class="w-full" />
        </UFormField>
        <UFormField :label="t('system.oidc.client_id')" required>
          <UInput v-model="form.clientId" autocomplete="off" class="w-full" />
        </UFormField>
        <UFormField
          :label="t('system.oidc.client_secret')"
          :description="secretNeeded ? undefined : t('system.oidc.client_secret_kept')"
          :required="secretNeeded"
        >
          <UInput v-model="form.clientSecret" type="password" autocomplete="new-password" class="w-full" />
        </UFormField>
        <UFormField :label="t('system.oidc.display_name')" :description="t('system.oidc.display_name_hint')" required>
          <UInput v-model="form.displayName" maxlength="60" class="w-full" />
        </UFormField>
        <div class="grid grid-cols-2 gap-2">
          <UFormField :label="t('system.oidc.group_claim')" :description="t('system.oidc.group_hint')">
            <UInput v-model="form.groupClaim" placeholder="groups" class="w-full" />
          </UFormField>
          <UFormField :label="t('system.oidc.group_value')">
            <UInput v-model="form.groupValue" class="w-full" />
          </UFormField>
        </div>
        <USwitch
          v-model="form.providerLogout"
          class="sm:col-span-2"
          :label="t('system.oidc.provider_logout')"
          :description="t('system.oidc.provider_logout_hint')"
        />
        <UFormField class="sm:col-span-2" :label="t('system.mfa.step_up.label')" :description="t('system.oidc.step_up_hint')">
          <UInput v-model="password" type="password" autocomplete="current-password" class="w-full sm:max-w-sm" />
        </UFormField>
        <div class="flex flex-wrap gap-2 sm:col-span-2">
          <UButton type="submit" icon="i-lucide-save" :label="t('system.oidc.save')" :loading="busy" :disabled="!canSave" />
          <UButton
            v-if="configured"
            color="error"
            variant="ghost"
            icon="i-lucide-trash-2"
            :label="t('system.oidc.remove.action')"
            :loading="busy"
            :disabled="!password || !settings.password_login"
            @click="remove"
          />
        </div>
      </form>
    </template>
  </section>
</template>
