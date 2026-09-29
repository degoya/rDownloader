<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import { PALETTE_FUSE, buildPaletteGroups, keepFocusOnClose, paletteOpen } from '@/composables/searchPalette'

/**
 * The search over the views and every setting (RD-170-15): Nuxt UI's command palette in its
 * modal. `UDashboardSearch` binds Ctrl/Cmd+K itself and opens on `UDashboardSearchButton` in
 * the sidebar; `/` opens it through the shortcut catalogue. Its own theme group stays off — the
 * theme is a preference with its own place in the sidebar footer and the interface settings.
 * Closing it returns the focus to where it was, unless a found field has just taken it.
 */
const { t } = useI18n()
const groups = computed(() => buildPaletteGroups(t))
const content = { onCloseAutoFocus: keepFocusOnClose }
</script>

<template>
  <UDashboardSearch
    v-model:open="paletteOpen"
    :groups="groups"
    :fuse="PALETTE_FUSE"
    :color-mode="false"
    :content="content"
    :title="t('nav.search.title')"
    :description="t('nav.search.description')"
    :placeholder="t('nav.search.placeholder')"
  />
</template>
