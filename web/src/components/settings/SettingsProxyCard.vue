<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Settings } from '@/api/types'
import SectionHeader from '@/components/SectionHeader.vue'
import { PLAIN } from '@/utils/numberInput'

const props = defineProps<{ modelValue: Settings }>()
const { t } = useI18n()

/// The list is edited as one address per line, which is how an operator thinks about it, and
/// stored as an array, which is what the API takes.
const trustedText = computed({
  get: () => (props.modelValue.trusted_proxies ?? []).join('\n'),
  set: (value: string) => {
    props.modelValue.trusted_proxies = value
      .split('\n')
      .map(entry => entry.trim())
      .filter(Boolean)
  }
})

/// The names a browser may call the service by, beyond addresses, localhost and the external
/// URL's host: one per line, like the proxy list above.
const allowedHostsText = computed({
  get: () => (props.modelValue.allowed_hosts ?? []).join('\n'),
  set: (value: string) => {
    props.modelValue.allowed_hosts = value
      .split('\n')
      .map(entry => entry.trim())
      .filter(Boolean)
  }
})

/** Empty input clears the override; the backend then keeps the built-in default port (RD-1120-21, from *General*). */
const uiPort = computed<number | null>({
  get: () => props.modelValue.ui_port ?? null,
  set: (value) => {
    const port = Number(value)
    props.modelValue.ui_port = Number.isFinite(port) && port > 0 ? Math.round(port) : null
  }
})

const cookieOptions = computed(() => [
  { value: 'auto', label: t('system.proxy.cookie.auto') },
  { value: 'always', label: t('system.proxy.cookie.always') },
  { value: 'never', label: t('system.proxy.cookie.never') }
])

/// The two half-configured shapes, shown before saving rather than only in `doctor`: both
/// produce a service that runs and then behaves in a way nobody would trace back to here.
const warning = computed(() => {
  const hasProxies = (props.modelValue.trusted_proxies ?? []).length > 0
  const hasUrl = Boolean(props.modelValue.external_url?.trim())
  if (hasUrl && !hasProxies) return t('system.proxy.warning_no_proxies')
  if (hasProxies && !hasUrl) return t('system.proxy.warning_no_url')
  return null
})
</script>

<template>
  <UCard as="section" data-settings-anchor="security.reverse_proxy">
    <SectionHeader :eyebrow="t('system.proxy.eyebrow')" :title="t('system.proxy.title')" :description="t('system.proxy.description')" />

    <div class="mt-4 grid gap-4">
      <div>
        <UFormField data-settings-anchor="security.ui_port" :label="t('settings.ui_port.label')" :description="t('settings.ui_port.description')">
          <UInputNumber v-model="uiPort" :min="1024" :max="65535" :format-options="PLAIN" :placeholder="t('settings.ui_port.placeholder')" class="mt-2 w-full" />
        </UFormField>
        <p class="mt-2 flex items-start gap-1.5 text-xs leading-5 text-warning">
          <UIcon name="i-lucide-rotate-cw" class="mt-0.5 size-3.5 shrink-0" />
          <span>{{ t('settings.ui_port.restart_hint') }}</span>
        </p>
      </div>
      <UFormField data-settings-anchor="security.external_url" :label="t('system.proxy.external_url')" :description="t('system.proxy.external_url_hint')">
        <UInput
          :model-value="modelValue.external_url ?? ''"
          placeholder="https://rd.example.com/downloads"
          class="w-full"
          @update:model-value="modelValue.external_url = String($event).trim() || null"
        />
      </UFormField>
      <UFormField data-settings-anchor="security.allowed_hosts" :label="t('system.proxy.allowed_hosts')" :description="t('system.proxy.allowed_hosts_hint')">
        <UTextarea v-model="allowedHostsText" :rows="2" placeholder="nas.lan&#10;rdownloader" class="w-full font-mono" />
      </UFormField>
      <UFormField :label="t('system.proxy.cookie_label')" :description="t('system.proxy.cookie_hint')">
        <USelect v-model="modelValue.cookie_security" :items="cookieOptions" class="w-full" />
      </UFormField>
      <UFormField :label="t('system.proxy.trusted')" :description="t('system.proxy.trusted_hint')">
        <UTextarea v-model="trustedText" :rows="3" placeholder="127.0.0.1&#10;10.0.0.0/8" class="w-full font-mono" />
      </UFormField>
    </div>

    <UAlert
      v-if="warning"
      class="mt-3"
      color="warning"
      icon="i-lucide-triangle-alert"
      :description="warning"
    />
  </UCard>
</template>
