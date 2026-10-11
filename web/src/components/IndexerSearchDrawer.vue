<script setup lang="ts">
import { nextTick, onMounted, onUnmounted, provide, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import IndexerSearchPanel from '@/components/IndexerSearchPanel.vue'
import { setIndexerSearchFocusAction } from '@/composables/indexerSearchFocus'
import { INDEXER_SEARCH_STATE, useIndexerSearch } from '@/composables/useIndexerSearch'
import { useIndexersStore } from '@/stores/indexers'

/**
 * The LinkGrabber's indexer search in a drawer (RD-1230-02, owner 2026-10-09).
 *
 * It sat in the page above the list as a card of its own and took the room the list needs; it is
 * a thing reached for, not looked at. The navbar button and `f` open it and put the keyboard in
 * the search field — or, without an enabled indexer, on the hint's link to where one is set up.
 * From the bottom like the indexer subscriptions' drawer (`IndexerReviewList`), because the hits
 * are a wide table that a side panel would squeeze, and the list behind stays where it was.
 *
 * It stays open after hits are taken: the flow is to read the hits, take several, page on, and
 * each row says itself that its hit arrived; closing is the reader's choice (Esc, `x`, outside).
 * The state belongs to this component, which lives as long as the view, not to the drawer's
 * content, which is unmounted while closed — reopened, the inputs and the hits are as they were.
 */
const { t } = useI18n()
const indexers = useIndexersStore()
const open = ref(false)
const panel = ref<{ focusField: () => void } | null>(null)
provide(INDEXER_SEARCH_STATE, useIndexerSearch())

function focusField(): void {
  panel.value?.focusField()
}

/** Opens the drawer and puts the keyboard in it; the navbar button and `f` both call this. */
async function openSearch(): Promise<void> {
  open.value = true
  await nextTick()
  focusField()
}

/** Reka would focus the first control of the drawer; the field is the one meant. */
function onOpenAutoFocus(event: Event): void {
  event.preventDefault()
  focusField()
}

// Opened before the indexer list has answered, the field is still disabled: once the answer is
// in, the field (or the hint's link) takes the keyboard.
watch(() => indexers.loaded, (loaded) => {
  if (loaded && open.value) void nextTick(focusField)
})

// Handed in once mounted: a request `f` left on another page opens the drawer right here.
onMounted(() => setIndexerSearchFocusAction(() => void openSearch()))
onUnmounted(() => setIndexerSearchFocusAction(null))

defineExpose({ openSearch })
</script>

<template>
  <UDrawer
    v-model:open="open"
    should-scale-background
    :title="t('linkgrabber.search.title')"
    :description="t('linkgrabber.search.drawer_hint')"
    :content="{ onOpenAutoFocus }"
  >
    <template #body>
      <div class="max-h-[80vh] overflow-y-auto p-3" data-testid="indexer-search-drawer">
        <IndexerSearchPanel ref="panel" />
      </div>
    </template>
  </UDrawer>
</template>
