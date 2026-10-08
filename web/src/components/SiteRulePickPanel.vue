<script setup lang="ts">
/**
 * Series pages whose releases wait for a choice, above the LinkGrabber's list (RD-1170-03).
 *
 * A two-stage site rule answers a pasted series page with its releases and resolves none of
 * them, because each costs a captcha. They wait here: a one-line header with the count, and the
 * pages themselves in a drawer — the review list pattern of `design.md`, so a list that grows does
 * not push the links somebody is reading. The paste that listed a page asked for the choice, so
 * it opens the drawer — whichever way it came: the LinkGrabber's own field, a copied link, the
 * browser extension or Click'n'Load (RD-1190-17).
 *
 * The drawer lives beside the header, not inside it: a header that disappears with the last list
 * used to take an open drawer with it, and the page stayed dimmed and unclickable. With no list
 * left the drawer closes instead.
 */
import { useToast } from '@nuxt/ui/composables'
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import SiteRulePickPage from '@/components/SiteRulePickPage.vue'
import { useSitePicksStore } from '@/stores/sitePicks'

const { t } = useI18n()
const toast = useToast()
const picks = useSitePicksStore()

const open = ref(false)
const waiting = computed(() => picks.pages.reduce((sum, page) => sum + page.entries.filter(entry => entry.state === 'pending').length, 0))

onMounted(async () => {
  const announced = picks.attach()
  await picks.refresh()
  if (announced && picks.pages.length) open.value = true
})
onUnmounted(() => {
  picks.detach()
  picks.stop()
})
watch(() => picks.asked, () => { open.value = true })
watch(() => picks.pages.length, (count) => { if (!count) open.value = false })

async function resolve(id: string, entries: number[]): Promise<void> {
  if (!await picks.resolve(id, entries) && picks.error) {
    toast.add({ title: picks.error, color: 'error', icon: 'i-lucide-circle-alert' })
  }
}
</script>

<template>
  <section v-if="picks.pages.length" class="border border-muted" data-testid="pick-panel">
    <div class="flex items-center gap-2 p-3">
      <div class="flex min-w-0 flex-1 items-center gap-2">
        <span class="text-sm font-medium text-highlighted">{{ t('linkgrabber.picks.title') }}</span>
        <UBadge color="primary" variant="subtle">{{ picks.pages.length }}</UBadge>
        <span class="truncate text-xs text-muted">{{ t('linkgrabber.picks.summary', { count: waiting }, waiting) }}</span>
      </div>
      <UButton
        size="xs"
        color="neutral"
        variant="outline"
        icon="i-lucide-panel-bottom-open"
        :label="t('linkgrabber.picks.open')"
        :title="t('linkgrabber.picks.open')"
        @click="open = true"
      />
    </div>
  </section>

  <UDrawer
    v-model:open="open"
    should-scale-background
    :title="t('linkgrabber.picks.title')"
    :description="t('linkgrabber.picks.hint')"
  >
    <template #body>
      <div class="max-h-[70vh] space-y-3 overflow-y-auto p-3">
        <p v-if="picks.error" role="alert" class="text-sm text-error">{{ picks.error }}</p>
        <SiteRulePickPage
          v-for="page in picks.pages"
          :key="page.id"
          :page="page"
          :busy="picks.busy.has(page.id)"
          @resolve="(entries: number[]) => void resolve(page.id, entries)"
          @cancel="void picks.cancel(page.id)"
          @discard="void picks.discard(page.id)"
        />
      </div>
    </template>
  </UDrawer>
</template>
