<script setup lang="ts">
import { useI18n } from 'vue-i18n'

/**
 * The hosters an account covers, folded under its row and read when first opened. Its trigger
 * stands at the left with a chevron, like the other folded parts of a card (RD-1260-05). The
 * filter is the one all open lists share, so it is the card's and comes in as a model.
 */
defineProps<{
  /** The hosters the filter lets through. */
  hosters: string[]
  /** Whether the account covers none at all, before any filter. */
  empty: boolean
  loading: boolean
}>()
const emit = defineEmits<{ open: [] }>()
const filter = defineModel<string>('filter', { required: true })

const { t } = useI18n()
</script>

<template>
  <UCollapsible class="mt-2" @update:open="(open: boolean) => open && emit('open')">
    <template #default="{ open }">
      <UButton size="xs" color="neutral" variant="ghost" class="-ml-2" :icon="open ? 'i-lucide-chevron-down' : 'i-lucide-chevron-right'" :aria-expanded="open" :label="t('network.hosters.toggle')" />
    </template>
    <template #content>
      <USeparator class="mb-2" />
      <div>
        <p class="mb-2 text-xs leading-5 text-muted">{{ t('network.hosters.description') }}</p>
        <UInput v-model="filter" icon="i-lucide-search" size="sm" :placeholder="t('network.hosters.filter_placeholder')" class="mb-2 w-full" />
        <p v-if="loading" class="text-xs text-muted">{{ t('network.hosters.loading') }}</p>
        <p v-else-if="empty" class="text-xs text-muted">{{ t('network.hosters.empty') }}</p>
        <div v-else class="flex max-h-48 flex-wrap gap-1 overflow-y-auto">
          <UBadge v-for="host in hosters" :key="host" color="neutral" variant="subtle" size="sm" class="font-mono">{{ host }}</UBadge>
        </div>
      </div>
    </template>
  </UCollapsible>
</template>
