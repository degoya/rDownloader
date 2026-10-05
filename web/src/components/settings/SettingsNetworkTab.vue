<script setup lang="ts">
import { computed, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { api } from '@/api/client'
import type { CreateProxyProfile, ProxyProfile, Settings } from '@/api/types'
import DataState from '@/components/DataState.vue'
import FormActions from '@/components/FormActions.vue'
import FormListLayout from '@/components/FormListLayout.vue'
import SectionHeader from '@/components/SectionHeader.vue'
import SettingsAuthProfilesCard from '@/components/settings/SettingsAuthProfilesCard.vue'
import SettingsReconnectCard from '@/components/settings/SettingsReconnectCard.vue'
import { useCopyName } from '@/composables/useCopyName'
import { useEditableList } from '@/composables/useEditableList'
import { useFormFocus } from '@/composables/useFormFocus'
import { subTabItems } from '@/composables/useSettingsSubTab'
import { NO_SELECTION, optionalSelection, selectionValue } from '@/utils/select'

/** Matches `validate_name` in `crates/rd-api-admin/src/config_handlers.rs`. */
const MAX_PROXY_NAME = 100

const settings = defineModel<Settings>({ required: true })
const proxies = defineModel<ProxyProfile[]>('proxies', { required: true })
/**
 * Owned by the settings view, which keeps it in the address (RD-180-15). Two list editors and
 * two cards made the page three tabs: the connection routes with the certificate they trust,
 * the sign-ins sent to hosts, and the router reconnect.
 */
const activeTab = defineModel<string>('subTab', { default: 'proxies' })
const props = defineProps<{
  /** True while the parent view is still fetching the proxy list (RD-104-07). */
  proxiesLoading?: boolean | undefined
  /** The proxy fetch's failure, so an unreachable service is not drawn as "none configured". */
  proxiesError?: string | null | undefined
}>()
const { t } = useI18n()
const tabItems = computed(() => subTabItems('network', t, { proxies: proxies.value.length }))
const formElement = ref<HTMLFormElement | null>(null)
const focusForm = useFormFocus(formElement)
const copyName = useCopyName()

/** The proxy form's own feedback, above the form rather than at the foot of the page (RD-150-11). */
const proxyMessage = ref<string | null>(null)
const deletingId = ref<string | null>(null)
/**
 * The profile the form was filled from while that copy is still unsaved (RD-190-22). The copy
 * carries no password — the browser never holds one — so the form asks for it again.
 */
const copiedFrom = ref<string | null>(null)
const proxyForm = reactive<CreateProxyProfile>({
  name: '',
  kind: 'http',
  endpoint: defaultEndpoint('http'),
  username: null,
  password: null
})
const proxyKindItems = [
  { label: 'HTTP', value: 'http' },
  { label: 'HTTPS', value: 'https' },
  { label: 'SOCKS5', value: 'socks5' }
]

function defaultEndpoint(kind: CreateProxyProfile['kind']): string {
  return `${kind === 'socks5' ? 'socks5h' : kind}://127.0.0.1:${kind === 'socks5' ? '1080' : '8080'}`
}

// Synchronous, so filling the form from a row — kind first, then its endpoint — keeps the row's
// endpoint instead of having it replaced by the default a moment later.
watch(() => proxyForm.kind, (kind) => {
  proxyForm.endpoint = defaultEndpoint(kind)
}, { flush: 'sync' })

const list = useEditableList<ProxyProfile, CreateProxyProfile>({
  list: proxies,
  create: body => api.POST('/api/v1/proxy-profiles', { body }),
  // An update without a password keeps the stored one (`update_proxy_profile`).
  update: (id, body) => api.PUT('/api/v1/proxy-profiles/{id}', { params: { path: { id } }, body }),
  destroy: id => api.DELETE('/api/v1/proxy-profiles/{id}', { params: { path: { id } } }),
  reset: () => {
    proxyForm.kind = 'http'
    proxyForm.endpoint = defaultEndpoint('http')
    proxyForm.name = ''
    proxyForm.username = null
    proxyForm.password = null
    copiedFrom.value = null
  },
  confirmDelete: proxy => ({
    title: t('settings.proxy.delete.title'),
    description: chosenHere(proxy.id)
      ? `${t('settings.proxy.delete.description', { name: proxy.name })} ${t('settings.proxy.delete.in_use_here')}`
      : t('settings.proxy.delete.description', { name: proxy.name }),
    confirmLabel: t('settings.proxy.delete.confirm'),
    confirmIcon: 'i-lucide-trash-2',
    destructive: true
  })
})
const { editingId, pending, error } = list

const proxyItems = computed(() => [
  { label: t('settings.proxy.direct'), value: NO_SELECTION },
  ...proxies.value.map(proxy => ({ label: `${proxy.name} · ${proxy.kind}`, value: proxy.id }))
])
const globalProxySelection = computed({
  get: () => optionalSelection(settings.value.global_proxy_profile_id),
  set: (value: string) => { settings.value.global_proxy_profile_id = selectionValue(value) }
})

/** Whether the settings on this page name the profile; the service then refuses the delete. */
function chosenHere(id: string): boolean {
  return settings.value.global_proxy_profile_id === id || settings.value.torrent_proxy_profile_id === id
}

async function saveProxy(): Promise<void> {
  if (!proxyForm.name || !proxyForm.endpoint) return
  proxyMessage.value = null
  const updating = editingId.value !== null
  const saved = await list.submit({
    ...proxyForm,
    username: proxyForm.username || null,
    password: proxyForm.password || null
  })
  if (saved) proxyMessage.value = updating ? t('settings.proxy.updated') : t('settings.proxy.created')
}

function fill(proxy: ProxyProfile, name: string): void {
  proxyForm.kind = proxy.kind
  proxyForm.endpoint = proxy.endpoint
  proxyForm.name = name
  proxyForm.username = proxy.username ?? null
  proxyForm.password = null
}

function editProxy(proxy: ProxyProfile): void {
  proxyMessage.value = null
  copiedFrom.value = null
  list.edit(proxy)
  fill(proxy, proxy.name)
  void focusForm()
}

/**
 * Fills the form with a copy of `proxy`, unsaved (RD-190-22), like a Usenet server's copy: the
 * service takes a username only together with its password, and the password never reaches the
 * browser, so the reader's own save creates the copy once the password is entered again.
 */
function duplicateProxy(proxy: ProxyProfile): void {
  proxyMessage.value = null
  list.reset()
  fill(proxy, copyName(proxy.name, proxies.value.map(entry => entry.name), MAX_PROXY_NAME))
  copiedFrom.value = proxy.name
  void focusForm()
}

async function deleteProxy(proxy: ProxyProfile): Promise<void> {
  proxyMessage.value = null
  deletingId.value = proxy.id
  const { removed } = await list.remove(proxy)
  deletingId.value = null
  if (removed) proxyMessage.value = t('settings.proxy.deleted')
}
</script>

<template>
  <div class="space-y-6">
    <header>
      <SectionHeader
        :eyebrow="t('settings.headers.network.eyebrow')"
        :title="t('settings.headers.network.title')"
        :description="t('settings.headers.network.description')"
        level="page"
      />
    </header>
    <UTabs
      v-model="activeTab"
      :items="tabItems"
      :unmount-on-hide="false"
      variant="pill"
      class="w-full"
      :ui="{ content: 'pt-4' }"
    >
      <template #proxies>
        <div class="space-y-6">
          <UCard as="section" data-settings-anchor="network.proxies">
            <FormListLayout :list-title="t('settings.proxy.list_title')" :count="proxies.length">
              <template #form>
                <SectionHeader
                  class="mb-4"
                  :eyebrow="t('settings.proxy.eyebrow')"
                  :title="editingId ? t('settings.proxy.edit_title') : t('settings.proxy.title')"
                  :description="t('settings.proxy.description')"
                  level="sub"
                />
                <UAlert v-if="error" class="mb-3" color="error" variant="subtle" icon="i-lucide-circle-alert" :description="error" />
                <UAlert v-if="proxyMessage" class="mb-3" color="success" variant="subtle" icon="i-lucide-circle-check" :description="proxyMessage" />
                <!-- The protocol rewrites the endpoint's scheme and port, so it stands first. -->
                <form ref="formElement" class="grid gap-3" @submit.prevent="saveProxy">
                  <UFormField :label="t('settings.proxy.kind_label')" :description="t('settings.proxy.kind_description')" required>
                    <USelect v-model="proxyForm.kind" :items="proxyKindItems" class="w-full" />
                  </UFormField>
                  <UFormField :label="t('settings.proxy.endpoint_label')" :description="t('settings.proxy.endpoint_description')" required>
                    <UInput v-model="proxyForm.endpoint" required class="w-full font-mono" placeholder="socks5h://127.0.0.1:1080" />
                  </UFormField>
                  <UFormField :label="t('settings.proxy.name_label')" :description="t('settings.proxy.name_description')" required>
                    <UInput v-model="proxyForm.name" required maxlength="100" :placeholder="t('settings.proxy.name_placeholder')" icon="i-lucide-route" class="w-full" />
                  </UFormField>
                  <UFormField :label="t('settings.proxy.username_label')" :description="t('settings.proxy.username_description')">
                    <UInput v-model="proxyForm.username" :placeholder="t('settings.proxy.username_placeholder')" class="w-full" />
                  </UFormField>
                  <UFormField
                    :label="t('settings.proxy.password_label')"
                    :description="copiedFrom && proxyForm.username ? t('settings.proxy.password_copy', { name: copiedFrom }) : t('settings.proxy.password_description')"
                    :required="Boolean(copiedFrom && proxyForm.username)"
                  >
                    <UInput
                      v-model="proxyForm.password"
                      type="password"
                      :placeholder="editingId ? t('settings.proxy.password_keep') : t('settings.proxy.password_placeholder')"
                      autocomplete="new-password"
                      class="w-full"
                    />
                  </UFormField>
                  <FormActions
                    :editing="editingId !== null"
                    :cancellable="editingId !== null || copiedFrom !== null"
                    :create-label="t('settings.proxy.create')"
                    :save-label="t('settings.proxy.save')"
                    :disabled="!proxyForm.name || !proxyForm.endpoint"
                    :loading="pending"
                    @cancel="list.reset"
                  />
                </form>
              </template>
              <template #list>
                <div class="divide-y divide-muted border border-muted">
                  <div
                    v-for="proxy in proxies"
                    :key="proxy.id"
                    class="flex flex-wrap items-center gap-3 p-3"
                    :class="editingId === proxy.id ? 'border-l-2 border-l-primary' : ''"
                    data-testid="proxy-row"
                  >
                    <UAvatar icon="i-lucide-waypoints" color="primary" />
                    <div class="min-w-0 flex-1 basis-40">
                      <p class="text-sm font-medium text-highlighted">{{ proxy.name }}</p>
                      <p class="truncate font-mono text-[11px] text-muted">{{ proxy.endpoint }}</p>
                    </div>
                    <div class="flex flex-wrap items-center gap-2">
                      <UBadge v-if="editingId === proxy.id" color="primary" variant="subtle">{{ t('common.editing') }}</UBadge>
                      <UBadge color="neutral" variant="subtle">{{ proxy.kind }}</UBadge>
                      <UIcon v-if="proxy.has_credentials" name="i-lucide-key-round" class="text-success" />
                      <UButton
                        size="xs"
                        color="neutral"
                        variant="ghost"
                        icon="i-lucide-copy-plus"
                        :label="t('common.actions.duplicate')"
                        :title="t('common.duplicate_hint')"
                        @click="duplicateProxy(proxy)"
                      />
                      <UButton size="xs" color="neutral" variant="ghost" icon="i-lucide-pencil" :aria-label="t('settings.proxy.edit_title')" :title="t('settings.proxy.edit_title')" @click="editProxy(proxy)" />
                      <UButton size="xs" color="error" variant="ghost" icon="i-lucide-trash-2" :aria-label="t('settings.proxy.delete.title')" :title="t('settings.proxy.delete.title')" :loading="deletingId === proxy.id" @click="deleteProxy(proxy)" />
                    </div>
                  </div>
                  <DataState :loading="props.proxiesLoading" :error="props.proxiesError" :empty="!proxies.length" variant="inline" class="p-5">
                    <p class="text-center text-sm text-muted">{{ t('settings.proxy.empty') }}</p>
                  </DataState>
                </div>
              </template>
            </FormListLayout>
          </UCard>

          <UCard as="section" data-settings-anchor="network.global_proxy" :ui="{ body: 'grid gap-4' }">
            <SectionHeader
              :eyebrow="t('settings.global_proxy.eyebrow')"
              :title="t('settings.global_proxy.title')"
              :description="t('settings.global_proxy.description')"
              level="sub"
            />
            <UFormField :label="t('settings.global_proxy.label')">
              <USelect v-model="globalProxySelection" :items="proxyItems" class="w-full" />
            </UFormField>
            <UFormField data-settings-anchor="network.custom_ca" :label="t('settings.custom_ca.label')" :description="t('settings.custom_ca.description')">
              <UTextarea v-model="settings.custom_ca_pem" :rows="7" class="w-full font-mono text-xs" placeholder="-----BEGIN CERTIFICATE-----" />
            </UFormField>
          </UCard>
        </div>
      </template>
      <template #auth>
        <SettingsAuthProfilesCard />
      </template>
      <template #reconnect>
        <SettingsReconnectCard v-model="settings" />
      </template>
    </UTabs>
  </div>
</template>
