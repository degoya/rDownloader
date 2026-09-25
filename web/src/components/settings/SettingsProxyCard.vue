<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Settings } from '@/api/types'
import SectionHeader from '@/components/SectionHeader.vue'

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
  <section class="mt-6 border border-muted bg-default p-5">
    <SectionHeader :eyebrow="t('system.proxy.eyebrow')" :title="t('system.proxy.title')" :description="t('system.proxy.description')" />

    <div class="mt-4 grid gap-4 lg:grid-cols-2">
      <UFormField :label="t('system.proxy.external_url')" :help="t('system.proxy.external_url_hint')">
        <UInput
          :model-value="modelValue.external_url ?? ''"
          placeholder="https://rd.example.com/downloads"
          class="w-full"
          @update:model-value="modelValue.external_url = String($event).trim() || null"
        />
      </UFormField>
      <UFormField :label="t('system.proxy.cookie_label')" :help="t('system.proxy.cookie_hint')">
        <USelect v-model="modelValue.cookie_security" :items="cookieOptions" class="w-full" />
      </UFormField>
      <UFormField
        class="lg:col-span-2"
        :label="t('system.proxy.trusted')"
        :help="t('system.proxy.trusted_hint')"
      >
        <UTextarea v-model="trustedText" :rows="3" placeholder="127.0.0.1&#10;10.0.0.0/8" class="w-full font-mono" />
      </UFormField>
    </div>

    <UAlert
      v-if="warning"
      class="mt-3"
      color="warning"
      variant="subtle"
      icon="i-lucide-triangle-alert"
      :description="warning"
    />
  </section>
</template>
