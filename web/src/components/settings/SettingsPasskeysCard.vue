<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useToast } from '@nuxt/ui/composables'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { components } from '@/api/schema'
import DataState from '@/components/DataState.vue'
import { useConfirm } from '@/composables/useConfirm'
import { PasskeyAbort, createCredential, passkeysSupported } from '@/webauthn'
import { formatMoment } from '@/utils/format'
import SectionHeader from '@/components/SectionHeader.vue'

type MfaCredential = components['schemas']['MfaCredential']

const { t } = useI18n()
const credentials = ref<MfaCredential[]>([])
const error = ref<string | null>(null)
const busy = ref(false)
const supported = passkeysSupported()

const confirm = useConfirm()
const toast = useToast()

/// Naming happens after the ceremony, so nobody has to invent a label before finding out
/// whether their authenticator would even produce a key.
const naming = ref<{ ceremonyId: string; credential: unknown } | null>(null)
const label = ref('')

const empty = computed(() => credentials.value.length === 0)
/** True until the first fetch settles: an absent list and an unfetched one look identical. */
const loading = ref(true)

onMounted(() => { void load() })

async function load(): Promise<void> {
  const response = await api.GET('/api/v1/mfa')
  loading.value = false
  if (response.data) {
    credentials.value = response.data.credentials.filter((entry) => entry.kind === 'webauthn')
    error.value = null
  } else {
    error.value = responseError(response)
  }
}

async function add(): Promise<void> {
  busy.value = true
  error.value = null
  const started = await api.POST('/api/v1/mfa/passkey')
  if (!started.data) {
    busy.value = false
    error.value = responseError(started)
    return
  }
  try {
    // The ceremony options are a W3C structure the browser parses, so the contract types them
    // as an opaque object; the shape assertion belongs here rather than in the schema.
    const options = (started.data.options as unknown as { publicKey: Record<string, unknown> })
      .publicKey
    naming.value = {
      ceremonyId: started.data.ceremony_id,
      credential: await createCredential(options)
    }
    label.value = defaultLabel()
  } catch (cause) {
    // A dismissed dialog is a decision, not a fault. Reporting it as an error would tell
    // somebody who just chose "cancel" that something went wrong.
    if (!(cause instanceof PasskeyAbort)) error.value = t('system.passkeys.failed')
  } finally {
    busy.value = false
  }
}

async function save(): Promise<void> {
  if (!naming.value) return
  busy.value = true
  const response = await api.POST('/api/v1/mfa/passkey/confirm', {
    body: {
      ceremony_id: naming.value.ceremonyId,
      label: label.value,
      credential: naming.value.credential as Record<string, never>
    }
  })
  busy.value = false
  if (!response.data) {
    error.value = responseError(response)
    return
  }
  naming.value = null
  await load()
  toast.add({ title: t('system.passkeys.added_toast'), color: 'success', icon: 'i-lucide-key-round' })
}

async function remove(credential: MfaCredential): Promise<void> {
  const confirmed = await confirm({
    title: t('system.passkeys.remove.title'),
    description: t('system.passkeys.remove.description', { label: credential.label }),
    confirmLabel: t('system.passkeys.remove.confirm'),
    confirmIcon: 'i-lucide-key-round',
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

/// A name the person will recognise later. The browser does not say which authenticator was
/// used, so this is a guess at the device rather than at the key — better than "Passkey 3".
function defaultLabel(): string {
  // `userAgentData` is the modern spelling and is missing from the DOM types, so it is read
  // defensively; `platform` is deprecated but still the only thing Safari and Firefox offer.
  const agent = navigator as Navigator & { userAgentData?: { platform?: string } }
  const platform = agent.userAgentData?.platform ?? navigator.platform
  return platform ? t('system.passkeys.default_label', { platform }) : t('system.passkeys.fallback_label')
}

</script>

<template>
  <section class="border border-muted bg-default p-5">
    <SectionHeader :eyebrow="t('system.passkeys.eyebrow')" :title="t('system.passkeys.title')" :description="t('system.passkeys.description')" />

    <UAlert v-if="error" class="mt-3" color="error" variant="subtle" :description="error" />
    <UAlert
      v-if="!supported"
      class="mt-3"
      color="neutral"
      variant="subtle"
      icon="i-lucide-info"
      :description="t('system.passkeys.unsupported')"
    />

    <DataState :loading="loading" :rows="2" class="mt-4" />

    <ul v-if="!loading && !empty" class="mt-4 divide-y divide-muted border border-muted">
      <li
        v-for="credential in credentials"
        :key="credential.id"
        class="flex flex-wrap items-center justify-between gap-3 p-3"
      >
        <div>
          <p class="text-sm font-medium text-highlighted">{{ credential.label }}</p>
          <p class="mt-1 text-xs text-muted">
            {{ t('system.passkeys.last_used', { moment: formatMoment(credential.last_used_at) || t('system.passkeys.never_used') }) }}
          </p>
        </div>
        <UButton
          size="sm"
          color="neutral"
          variant="ghost"
          icon="i-lucide-trash-2"
          :label="t('system.passkeys.remove.action')"
          @click="remove(credential)"
        />
      </li>
    </ul>

    <!-- The ceremony is done; all that is left is a name for the list. -->
    <form v-if="naming" class="mt-4 flex flex-wrap items-end gap-2" @submit.prevent="save">
      <UFormField class="flex-1" :label="t('system.passkeys.name_label')" :help="t('system.passkeys.name_hint')">
        <UInput v-model="label" maxlength="60" autofocus class="w-full" />
      </UFormField>
      <UButton type="submit" icon="i-lucide-check" :label="t('system.passkeys.save')" :loading="busy" />
    </form>

    <UButton
      v-else
      class="mt-4"
      icon="i-lucide-key-round"
      :label="t('system.passkeys.add')"
      :loading="busy"
      :disabled="!supported"
      @click="add"
    />
  </section>
</template>
