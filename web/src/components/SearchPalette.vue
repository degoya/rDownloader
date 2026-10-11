<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { queueSearchGroups, usePaletteQueueSearch } from '@/composables/paletteQueueSearch'
import { PALETTE_FUSE, buildPaletteGroups, keepFocusOnClose, paletteOpen } from '@/composables/searchPalette'

/**
 * The search over the views and every setting (RD-170-15): Nuxt UI's command palette in its
 * modal. `UDashboardSearch` binds Ctrl/Cmd+K itself and opens on `UDashboardSearchButton` in
 * the sidebar; `/` opens it through the shortcut catalogue. Its own theme group stays off — the
 * theme is a preference with its own place in the sidebar footer and the interface settings.
 * Closing it returns the focus to where it was, unless a found field has just taken it. What is
 * typed also asks the server for the queue's packages and files by name (RD-1240-14); they follow
 * the views, and choosing one opens the download list on its row.
 */
const { t } = useI18n()
const term = ref('')
const { hits, loading } = usePaletteQueueSearch(term)
const groups = computed(() => {
  const [views, ...rest] = buildPaletteGroups(t)
  return [...(views ? [views] : []), ...queueSearchGroups(t, hits.value), ...rest]
})
// A palette opened again starts from nothing, as its field does.
watch(paletteOpen, (open) => { if (!open) term.value = '' })
const content = { onCloseAutoFocus: keepFocusOnClose }
</script>

<template>
  <UDashboardSearch
    v-model:open="paletteOpen"
    v-model:search-term="term"
    :groups="groups"
    :fuse="PALETTE_FUSE"
    :loading="loading"
    :color-mode="false"
    :content="content"
    :title="t('nav.search.title')"
    :description="t('nav.search.description')"
    :placeholder="t('nav.search.placeholder')"
  />
</template>
