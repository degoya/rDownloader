<script setup lang="ts">
/**
 * The LinkGrabber's own page (RD-1120-23): what it drops before a link reaches it, how it opens
 * DLC containers, and whether it groups copies of one file as mirrors. The excluded domains and
 * DLC were the "Collector" sub-tab of *Storage & rules*, mirror detection a switch on *General*;
 * the display switches for its rows are on *Interface › Display*.
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Settings } from '@/api/types'
import SectionHeader from '@/components/SectionHeader.vue'

const settings = defineModel<Settings>({ required: true })
const { t } = useI18n()

/** An empty field means "the built-in service", which the API stores as `null`. */
const endpoint = computed({
  get: () => settings.value.dlc_service_endpoint ?? '',
  set: (value: string) => {
    settings.value.dlc_service_endpoint = value.trim() ? value : null
  }
})

const FORMAT_KEYS = ['one_per_line', 'comments', 'www', 'subdomains'] as const
</script>

<template>
  <div class="space-y-6">
    <header>
      <SectionHeader
        :eyebrow="t('settings.headers.linkgrabber.eyebrow')"
        :title="t('settings.headers.linkgrabber.title')"
        :description="t('settings.headers.linkgrabber.description')"
        level="page"
      />
    </header>
    <UCard as="section" data-settings-anchor="linkgrabber.blocklist" :ui="{ body: 'grid gap-4 md:grid-cols-2' }">
      <div class="space-y-4">
        <div>
          <SectionHeader
            :eyebrow="t('settings.collector.eyebrow')"
            :title="t('settings.collector.title')"
            :description="t('settings.collector.description')"
            level="sub"
          />
        </div>
        <UFormField
          data-settings-anchor="linkgrabber.excluded_domains"
          :label="t('settings.collector.excluded_domains.label')"
          :description="t('settings.collector.excluded_domains.description')"
        >
          <UInput v-model="settings.excluded_domains_file" icon="i-lucide-shield-ban" placeholder="/config/excluded_domains.txt" class="w-full font-mono" />
        </UFormField>
      </div>
      <div class="space-y-3 border border-muted bg-elevated p-4">
        <p class="text-sm font-medium text-highlighted">{{ t('settings.collector.format.title') }}</p>
        <ul class="space-y-1.5 text-xs leading-5 text-muted">
          <li v-for="key in FORMAT_KEYS" :key="key" class="flex items-start gap-2">
            <UIcon name="i-lucide-dot" class="mt-0.5 size-4 shrink-0 text-primary" />
            <span>{{ t(`settings.collector.format.${key}`) }}</span>
          </li>
        </ul>
        <pre class="overflow-x-auto border border-muted bg-default p-3 font-mono text-2xs leading-5 text-muted"># {{ t('settings.collector.format.sample_comment') }}
example.com
www.tracker.example
ads.example.org</pre>
      </div>
    </UCard>
    <UCard as="section" data-settings-anchor="linkgrabber.dlc">
      <div class="space-y-4">
        <div>
          <SectionHeader
            :eyebrow="t('settings.collector.dlc.eyebrow')"
            :title="t('settings.collector.dlc.title')"
            :description="t('settings.collector.dlc.description')"
            level="sub"
          />
        </div>
        <UFormField :label="t('settings.collector.dlc.enabled.label')" orientation="horizontal" class="border-t border-muted pt-4">
          <template #description>
            {{ t('settings.collector.dlc.enabled.description') }}
            <span v-if="settings.dlc_service_enabled" class="mt-1 block text-warning">{{ t('settings.collector.dlc.enabled.warning') }}</span>
          </template>
          <USwitch v-model="settings.dlc_service_enabled" />
        </UFormField>
        <UFormField
          :label="t('settings.collector.dlc.endpoint.label')"
          :description="t('settings.collector.dlc.endpoint.description')"
        >
          <UInput
            v-model="endpoint"
            icon="i-lucide-link"
            :disabled="!settings.dlc_service_enabled"
            placeholder="http://service.jdownloader.org/dlcrypt/service.php"
            class="w-full font-mono"
          />
        </UFormField>
      </div>
    </UCard>

    <UCard as="section" :ui="{ body: 'grid gap-4' }">
      <UFormField data-settings-anchor="linkgrabber.mirrors" :label="t('settings.mirrors.label')" :description="t('settings.mirrors.description')" orientation="horizontal">
        <USwitch v-model="settings.mirror_detection" />
      </UFormField>
    </UCard>
  </div>
</template>
