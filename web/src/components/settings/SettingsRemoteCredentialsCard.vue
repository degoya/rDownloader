<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { RemoteCredential, RemoteProtocol, Settings, SshHostKey } from '@/api/types'
import SectionHeader from '@/components/SectionHeader.vue'
import { useConfirm } from '@/composables/useConfirm'
import {
  DEFAULT_PORTS,
  type RemoteCredentialForm,
  authModesFor,
  emptyForm,
  endpointLabel,
  formFor,
  hostKeyId,
  useRemoteCredentials
} from '@/composables/useRemoteCredentials'
import { formatMoment } from '@/utils/format'

const props = defineProps<{ settings: Settings }>()
const emit = defineEmits<{ message: [string], error: [string] }>()
/** The tunables belong to the parent's settings object, saved with the rest of the tab. */
const settings = computed(() => props.settings)
const { t } = useI18n()
const confirm = useConfirm()

const {
  credentials, hostKeys, loading, pending, busyId, incomplete,
  refresh, create, update, test, remove, trustHostKey, forgetHostKey
} = useRemoteCredentials((event, text) => {
  if (event === 'message') emit('message', text)
  else emit('error', text)
})

const form = reactive<RemoteCredentialForm>(emptyForm())
const editingId = ref<string | null>(null)
const clearPrivateKey = ref(false)

/** A host key the last test reported; confirming it is an explicit, separate decision. */
const pendingKey = ref<{
  changed: boolean
  host: string
  port: number
  algorithm: string
  fingerprint: string
  stored: string | undefined
} | null>(null)

const protocolItems = computed(() =>
  (['ftp', 'ftps', 'ftps_implicit', 'sftp'] as const).map(value => ({
    label: t(`remote.protocols.${value}`),
    value: value satisfies RemoteProtocol
  }))
)
const authModeItems = computed(() =>
  authModesFor(form.protocol).map(value => ({ label: t(`remote.auth_modes.${value}`), value }))
)
const portPlaceholder = computed(() =>
  t('remote.credentials.port_default', { port: DEFAULT_PORTS[form.protocol] })
)
const isFtp = computed(() => form.protocol !== 'sftp')
const usesPassword = computed(() => form.auth_mode === 'password')
const usesKey = computed(() => form.auth_mode === 'private_key')
const needsUsername = computed(() => form.auth_mode !== 'anonymous')

/**
 * A new login needs the credential its mode uses; an edit may keep the stored one.
 * Mirrors the server's `validate_auth` so the button does not offer a request that fails.
 */
const canSubmit = computed(() => {
  if (!form.name.trim() || !form.host.trim()) return false
  if (needsUsername.value && !form.username.trim()) return false
  if (editingId.value) return true
  if (usesPassword.value) return form.secret.trim().length > 0
  if (usesKey.value) return form.private_key.trim().length > 0
  return true
})

onMounted(() => void refresh())

/** Keeps the mode valid when the protocol changes: FTP has no keys or agents. */
function onProtocolChange(): void {
  if (!authModesFor(form.protocol).includes(form.auth_mode)) {
    form.auth_mode = form.protocol === 'sftp' ? 'password' : 'anonymous'
  }
}

function reset(): void {
  Object.assign(form, emptyForm())
  editingId.value = null
  clearPrivateKey.value = false
}

function startEdit(credential: RemoteCredential): void {
  Object.assign(form, formFor(credential))
  editingId.value = credential.id
  clearPrivateKey.value = false
}

async function submit(): Promise<void> {
  const id = editingId.value
  const ok = id ? await update(id, form, clearPrivateKey.value) : await create(form)
  if (!ok) return
  emit('message', t(id ? 'remote.credentials.save' : 'remote.credentials.create'))
  reset()
}

async function runTest(credential: RemoteCredential): Promise<void> {
  const failure = await test(credential.id)
  if (!failure) return emit('message', t('remote.credentials.test_ok'))
  // An unconfirmed host key is the normal first answer for a new SFTP server, so it opens
  // the confirmation rather than being reported as a plain error.
  const unknown = failure.code === 'sftp.host_key_unknown'
  const changed = failure.code === 'sftp.host_key_changed'
  if ((unknown || changed) && failure.params.fingerprint) {
    pendingKey.value = {
      changed,
      host: failure.params.host ?? credential.host,
      port: Number(failure.params.port ?? credential.port),
      algorithm: failure.params.algorithm ?? 'ssh-ed25519',
      fingerprint: failure.params.fingerprint,
      stored: failure.params.stored_fingerprint
    }
    return
  }
  emit('error', t(`server.codes.${failure.code}`, failure.params))
}

async function confirmPendingKey(): Promise<void> {
  const key = pendingKey.value
  if (!key) return
  const ok = await trustHostKey({
    host: key.host,
    port: key.port,
    algorithm: key.algorithm,
    fingerprint: key.fingerprint
  })
  if (ok) pendingKey.value = null
}

async function confirmRemove(credential: RemoteCredential): Promise<void> {
  const confirmed = await confirm({
    title: t('remote.credentials.delete'),
    description: t('remote.credentials.delete_confirm'),
    confirmLabel: t('remote.credentials.delete'),
    destructive: true
  })
  if (!confirmed) return
  if (editingId.value === credential.id) reset()
  await remove(credential.id)
}

async function confirmForget(key: SshHostKey): Promise<void> {
  const confirmed = await confirm({
    title: t('remote.host_keys.forget'),
    description: t('remote.host_keys.forget_confirm'),
    confirmLabel: t('remote.host_keys.forget'),
    destructive: true
  })
  if (confirmed) await forgetHostKey(key)
}
</script>

<template>
  <section class="border border-muted bg-default p-5">
    <div class="mb-4 flex items-start justify-between">
      <div>
        <SectionHeader :eyebrow="t('remote.credentials.title')" :title="t('remote.title')" :description="t('remote.description')" level="sub" />
      </div>
      <UBadge color="neutral" variant="outline">{{ credentials.length }}</UBadge>
    </div>

    <div v-if="pendingKey" class="mb-4 border p-4" :class="pendingKey.changed ? 'border-error bg-error/5' : 'border-warning bg-warning/5'">
      <p class="text-sm font-medium text-highlighted">
        {{ t(pendingKey.changed ? 'remote.host_keys.changed_title' : 'remote.host_keys.unknown_title') }}
      </p>
      <p class="mt-1 max-w-3xl text-xs leading-5 text-muted">
        {{ t(pendingKey.changed ? 'remote.host_keys.changed_description' : 'remote.host_keys.unknown_description') }}
      </p>
      <dl class="mt-3 space-y-1 text-xs">
        <div class="flex gap-2">
          <dt class="w-32 shrink-0 text-muted">{{ t('remote.host_keys.endpoint') }}</dt>
          <dd class="font-mono">{{ pendingKey.host }}:{{ pendingKey.port }} ({{ pendingKey.algorithm }})</dd>
        </div>
        <div class="flex gap-2">
          <dt class="w-32 shrink-0 text-muted">{{ t('remote.host_keys.offered') }}</dt>
          <dd class="break-all font-mono">{{ pendingKey.fingerprint }}</dd>
        </div>
        <div v-if="pendingKey.stored" class="flex gap-2">
          <dt class="w-32 shrink-0 text-muted">{{ t('remote.host_keys.stored') }}</dt>
          <dd class="break-all font-mono text-muted">{{ pendingKey.stored }}</dd>
        </div>
      </dl>
      <div class="mt-3 flex gap-2">
        <UButton
          size="xs"
          :color="pendingKey.changed ? 'error' : 'primary'"
          icon="i-lucide-shield-check"
          :label="t('remote.host_keys.confirm')"
          :loading="pending"
          @click="confirmPendingKey"
        />
        <UButton size="xs" color="neutral" variant="ghost" :label="t('remote.host_keys.reject')" @click="pendingKey = null" />
      </div>
    </div>

    <div class="grid gap-3 sm:grid-cols-2">
      <UFormField :label="t('remote.credentials.name')">
        <UInput v-model="form.name" maxlength="100" :placeholder="t('remote.credentials.name_placeholder')" icon="i-lucide-server" class="w-full" />
      </UFormField>
      <UFormField :label="t('remote.credentials.protocol')">
        <USelect v-model="form.protocol" :items="protocolItems" class="w-full" @update:model-value="onProtocolChange" />
      </UFormField>
      <UFormField :label="t('remote.credentials.host')">
        <UInput v-model="form.host" class="w-full font-mono" :placeholder="t('remote.credentials.host_placeholder')" />
      </UFormField>
      <UFormField :label="t('remote.credentials.port')">
        <UInput v-model="form.port" inputmode="numeric" class="w-full font-mono" :placeholder="portPlaceholder" />
      </UFormField>
      <UFormField :label="t('remote.credentials.auth_mode')">
        <USelect v-model="form.auth_mode" :items="authModeItems" class="w-full" />
      </UFormField>
      <UFormField v-if="needsUsername" :label="t('remote.credentials.username')">
        <UInput v-model="form.username" class="w-full" />
      </UFormField>
      <UFormField
        v-if="usesPassword"
        class="sm:col-span-2"
        :label="t('remote.credentials.password')"
        :description="editingId ? t('remote.credentials.password_keep') : t('remote.credentials.secrets_note')"
      >
        <UInput v-model="form.secret" type="password" class="w-full" />
      </UFormField>
      <UFormField
        v-if="usesKey"
        class="sm:col-span-2"
        :label="t('remote.credentials.private_key')"
        :description="editingId ? t('remote.credentials.private_key_keep') : t('remote.credentials.secrets_note')"
      >
        <UTextarea v-model="form.private_key" :rows="4" class="w-full font-mono text-xs" :placeholder="t('remote.credentials.private_key_placeholder')" />
      </UFormField>
      <UFormField
        v-if="usesKey"
        :label="t('remote.credentials.passphrase')"
        :description="editingId ? t('remote.credentials.passphrase_keep') : ''"
      >
        <UInput v-model="form.passphrase" type="password" class="w-full" />
      </UFormField>
      <UCheckbox v-if="usesKey && editingId" v-model="clearPrivateKey" :label="t('remote.credentials.clear_private_key')" />
      <UCheckbox v-if="isFtp" v-model="form.passive" :label="t('remote.credentials.passive')" :description="t('remote.credentials.passive_hint')" />
      <UCheckbox v-model="form.enabled" :label="t('remote.credentials.enabled')" />
      <div class="flex gap-2 sm:col-span-2">
        <UButton
          type="button"
          icon="i-lucide-plus"
          :label="editingId ? t('remote.credentials.save') : t('remote.credentials.create')"
          :disabled="!canSubmit"
          :loading="pending"
          @click="submit"
        />
        <UButton v-if="editingId" type="button" color="neutral" variant="ghost" :label="t('remote.credentials.cancel')" @click="reset" />
      </div>
    </div>

    <div class="mt-4 divide-y divide-muted border border-muted">
      <div v-for="credential in credentials" :key="credential.id" class="flex flex-wrap items-center gap-3 p-3">
        <span class="grid size-8 place-items-center bg-elevated text-primary">
          <UIcon :name="credential.protocol === 'sftp' ? 'i-lucide-shield' : 'i-lucide-folder-symlink'" />
        </span>
        <div class="min-w-0 flex-1">
          <p class="text-sm font-medium text-highlighted">{{ credential.name }}</p>
          <p class="truncate font-mono text-[11px] text-muted">{{ endpointLabel(credential) }}</p>
        </div>
        <UBadge color="neutral" variant="subtle">{{ t(`remote.protocols.${credential.protocol}`) }}</UBadge>
        <UBadge color="neutral" variant="outline">{{ t(`remote.auth_modes.${credential.auth_mode}`) }}</UBadge>
        <UBadge
          v-if="incomplete.some(item => item.id === credential.id)"
          color="warning"
          variant="subtle"
        >{{ t('remote.credentials.incomplete') }}</UBadge>
        <UBadge v-else-if="!credential.enabled" color="neutral" variant="outline">{{ t('remote.credentials.enabled') }}</UBadge>
        <UButton size="xs" color="neutral" variant="ghost" icon="i-lucide-plug-zap" :label="t('remote.credentials.test')" :loading="busyId === credential.id" @click="runTest(credential)" />
        <UButton size="xs" color="neutral" variant="ghost" icon="i-lucide-pencil" :label="t('remote.credentials.edit_title')" @click="startEdit(credential)" />
        <UButton size="xs" color="error" variant="ghost" icon="i-lucide-trash-2" :label="t('remote.credentials.delete')" :loading="busyId === credential.id" @click="confirmRemove(credential)" />
      </div>
      <p v-if="!loading && !credentials.length" class="p-5 text-center text-sm text-muted">{{ t('remote.credentials.empty') }}</p>
    </div>

    <div class="mt-6 border-t border-muted pt-4">
      <p class="text-sm font-medium text-highlighted">{{ t('remote.settings.title') }}</p>
      <div class="mt-3 grid gap-3 sm:grid-cols-2">
        <UFormField :label="t('remote.settings.max_parallel')" :description="t('remote.settings.max_parallel_hint')">
          <UInput v-model.number="settings.remote_max_parallel" type="number" min="1" max="8" icon="i-lucide-layers" class="w-full" />
        </UFormField>
        <UFormField :label="t('remote.settings.timeout')" :description="t('remote.settings.timeout_hint')">
          <UInput v-model.number="settings.remote_timeout_seconds" type="number" min="5" max="600" icon="i-lucide-timer" class="w-full" />
        </UFormField>
        <UCheckbox
          v-model="settings.remote_ssh_auto_trust"
          class="sm:col-span-2"
          :label="t('remote.settings.ssh_auto_trust')"
          :description="t('remote.settings.ssh_auto_trust_hint')"
        />
      </div>
    </div>

    <div class="mt-6">
      <p class="text-sm font-medium text-highlighted">{{ t('remote.host_keys.title') }}</p>
      <p class="mt-1 max-w-3xl text-xs leading-5 text-muted">{{ t('remote.host_keys.description') }}</p>
      <div class="mt-3 divide-y divide-muted border border-muted">
        <div v-for="key in hostKeys" :key="hostKeyId(key)" class="flex flex-wrap items-center gap-3 p-3">
          <UIcon name="i-lucide-key-round" class="text-success" />
          <div class="min-w-0 flex-1">
            <p class="font-mono text-xs text-highlighted">{{ key.host }}:{{ key.port }}</p>
            <p class="truncate break-all font-mono text-[11px] text-muted">{{ key.fingerprint }}</p>
          </div>
          <UBadge color="neutral" variant="subtle">{{ key.algorithm }}</UBadge>
          <span class="text-[11px] text-muted">{{ formatMoment(key.first_seen) }}</span>
          <UButton size="xs" color="error" variant="ghost" icon="i-lucide-trash-2" :label="t('remote.host_keys.forget')" :loading="busyId === hostKeyId(key)" @click="confirmForget(key)" />
        </div>
        <p v-if="!loading && !hostKeys.length" class="p-5 text-center text-sm text-muted">{{ t('remote.host_keys.empty') }}</p>
      </div>
    </div>
  </section>
</template>
