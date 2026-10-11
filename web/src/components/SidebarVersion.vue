<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import { BUILD_VERSION, serviceVersion } from '@/composables/serviceVersion'

/**
 * The running version at the foot of the sidebar, always on screen and leading to *About*
 * (RD-1250-06). It left the status bar, where it and the dedication pushed the selection's
 * count and size out of view. The service's answer wins; until it has come, the version this
 * page was built as stands in, so the line is never empty.
 */
const props = defineProps<{ collapsed?: boolean }>()
const { t } = useI18n()

const version = computed(() => serviceVersion.value ?? BUILD_VERSION)
/** The 64 px rail keeps 32 px between its paddings: room for "1.24", not for "v1.24.0". */
const shortVersion = computed(() => version.value.split('.').slice(0, 2).join('.'))
const title = computed(() => t('nav.version.sidebar_title', { version: version.value }))
</script>

<template>
  <UTooltip :text="title">
    <ULink
      to="/settings/about"
      raw
      exact
      data-testid="sidebar-version"
      :data-collapsed="props.collapsed ? 'true' : 'false'"
      class="flex rounded-sm py-1 font-mono text-xs focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary"
      :class="props.collapsed ? 'justify-center' : 'px-2'"
      active-class="text-highlighted"
      inactive-class="text-muted hover:text-highlighted"
    >
      <!-- The visible text stays inside the accessible name (WCAG 2.5.3): the short form is
           part of the full one, which the hidden part spells out with where the link goes. -->
      <template v-if="props.collapsed">
        <span aria-hidden="true">{{ shortVersion }}</span>
        <span class="visually-hidden">v{{ version }} – {{ t('nav.version.sidebar_about') }}</span>
      </template>
      <template v-else>
        <span class="min-w-0 truncate">v{{ version }}</span>
        <span class="visually-hidden"> – {{ t('nav.version.sidebar_about') }}</span>
      </template>
    </ULink>
  </UTooltip>
</template>
