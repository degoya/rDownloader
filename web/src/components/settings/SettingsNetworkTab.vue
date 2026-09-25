<script setup lang="ts">
import { computed, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { CreateProxyProfile, ProxyProfile, Settings } from '@/api/types'
import DataState from '@/components/DataState.vue'
import SectionHeader from '@/components/SectionHeader.vue'
import SettingsAuthProfilesCard from '@/components/settings/SettingsAuthProfilesCard.vue'
import SettingsReconnectCard from '@/components/settings/SettingsReconnectCard.vue'
import { NO_SELECTION, optionalSelection, selectionValue } from '@/utils/select'

const settings = defineModel<Settings>({ required: true })
const proxies = defineModel<ProxyProfile[]>('proxies', { required: true })
const props = defineProps<{
  /** True while the parent view is still fetching the proxy list (RD-104-07). */
  proxiesLoading?: boolean | undefined
  /** The proxy fetch's failure, so an unreachable service is not drawn as "none configured". */
  proxiesError?: string | null | undefined
}>()
const emit = defineEmits<{ message: [string], error: [string] }>()
const { t } = useI18n()

const proxyPending = ref(false)
const proxyForm = reactive<CreateProxyProfile>({
  name: '',
  kind: 'http',
  endpoint: 'http://127.0.0.1:8080',
  username: null,
  password: null
})
const proxyKindItems = [
  { label: 'HTTP', value: 'http' },
  { label: 'HTTPS', value: 'https' },
  { label: 'SOCKS5', value: 'socks5' }
]
watch(() => proxyForm.kind, (kind) => {
  const port = kind === 'socks5' ? '1080' : '8080'
  proxyForm.endpoint = `${kind === 'socks5' ? 'socks5h' : kind}://127.0.0.1:${port}`
})

const proxyItems = computed(() => [
  { label: t('settings.proxy.direct'), value: NO_SELECTION },
  ...proxies.value.map(proxy => ({ label: `${proxy.name} · ${proxy.kind}`, value: proxy.id }))
])
const globalProxySelection = computed({
  get: () => optionalSelection(settings.value.global_proxy_profile_id),
  set: (value: string) => { settings.value.global_proxy_profile_id = selectionValue(value) }
})

async function createProxy(): Promise<void> {
  proxyPending.value = true
  const body: CreateProxyProfile = {
    ...proxyForm,
    username: proxyForm.username || null,
    password: proxyForm.password || null
  }
  const response = await api.POST('/api/v1/proxy-profiles', { body })
  proxyPending.value = false
  if (!response.data) return void emit('error', responseError(response))
  proxies.value = [...proxies.value, response.data]
  proxyForm.name = ''
  proxyForm.username = null
  proxyForm.password = null
  emit('message', t('settings.proxy.created'))
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
    <section class="border border-muted bg-default p-5">
      <div class="mb-4 flex items-start justify-between">
        <div>
          <SectionHeader
            :eyebrow="t('settings.proxy.eyebrow')"
            :title="t('settings.proxy.title')"
            :description="t('settings.proxy.description')"
            level="sub"
          />
        </div>
        <UBadge color="neutral" variant="outline">{{ proxies.length }}</UBadge>
      </div>
      <div class="grid gap-3 sm:grid-cols-2">
        <UFormField :label="t('settings.proxy.name_label')" :description="t('settings.proxy.name_description')">
          <UInput v-model="proxyForm.name" maxlength="100" :placeholder="t('settings.proxy.name_placeholder')" icon="i-lucide-route" class="w-full" />
        </UFormField>
        <UFormField :label="t('settings.proxy.kind_label')" :description="t('settings.proxy.kind_description')">
          <USelect v-model="proxyForm.kind" :items="proxyKindItems" class="w-full" />
        </UFormField>
        <UFormField class="sm:col-span-2" :label="t('settings.proxy.endpoint_label')" :description="t('settings.proxy.endpoint_description')">
          <UInput v-model="proxyForm.endpoint" class="w-full font-mono" placeholder="socks5h://127.0.0.1:1080" />
        </UFormField>
        <UFormField :label="t('settings.proxy.username_label')" :description="t('settings.proxy.username_description')">
          <UInput v-model="proxyForm.username" :placeholder="t('settings.proxy.username_placeholder')" class="w-full" />
        </UFormField>
        <UFormField :label="t('settings.proxy.password_label')" :description="t('settings.proxy.password_description')">
          <UInput v-model="proxyForm.password" type="password" :placeholder="t('settings.proxy.password_placeholder')" class="w-full" />
        </UFormField>
        <UButton type="button" class="sm:col-span-2" icon="i-lucide-plus" :label="t('settings.proxy.create')" :disabled="!proxyForm.name || !proxyForm.endpoint" :loading="proxyPending" @click="createProxy" />
      </div>
      <div class="mt-4 divide-y divide-muted border border-muted">
        <div v-for="proxy in proxies" :key="proxy.id" class="flex items-center gap-3 p-3">
          <span class="grid size-8 place-items-center bg-elevated text-primary"><UIcon name="i-lucide-waypoints" /></span>
          <div class="min-w-0 flex-1">
            <p class="text-sm font-medium text-highlighted">{{ proxy.name }}</p>
            <p class="truncate font-mono text-[11px] text-muted">{{ proxy.endpoint }}</p>
          </div>
          <UBadge color="neutral" variant="subtle">{{ proxy.kind }}</UBadge>
          <UIcon v-if="proxy.has_credentials" name="i-lucide-key-round" class="text-success" />
        </div>
        <DataState :loading="props.proxiesLoading" :error="props.proxiesError" :empty="!proxies.length" variant="inline" class="p-5">
          <p class="text-center text-sm text-muted">{{ t('settings.proxy.empty') }}</p>
        </DataState>
      </div>
    </section>

    <section class="grid gap-4 border border-muted bg-default p-5 md:grid-cols-2">
      <div>
        <SectionHeader
          :eyebrow="t('settings.global_proxy.eyebrow')"
          :title="t('settings.global_proxy.title')"
          :description="t('settings.global_proxy.description')"
          level="sub"
        />
        <UFormField class="mt-3" :label="t('settings.global_proxy.label')">
          <USelect v-model="globalProxySelection" :items="proxyItems" class="w-full" />
        </UFormField>
      </div>
      <UFormField :label="t('settings.custom_ca.label')" :description="t('settings.custom_ca.description')">
        <UTextarea v-model="settings.custom_ca_pem" :rows="7" class="w-full font-mono text-xs" placeholder="-----BEGIN CERTIFICATE-----" />
      </UFormField>
    </section>

    <SettingsAuthProfilesCard
      @message="(text: string) => emit('message', text)"
      @error="(text: string) => emit('error', text)"
    />

    <SettingsReconnectCard v-model="settings" />
  </div>
</template>
