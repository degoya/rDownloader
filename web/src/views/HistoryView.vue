<script setup lang="ts">
/**
 * The download history (RD-1100-04): what was downloaded, findable after the package left
 * the queue — removed by hand, cleaned up automatically, or never finished.
 *
 * Row conventions follow `design.md` and the audit log: filters above the list, the count of
 * what the filters match, details (the source addresses) behind the chevron pair. "Add again"
 * puts an entry's sources back into the LinkGrabber, where the online check and the review
 * apply as to a pasted link. The clear is the shared data-reset control, with its count and
 * its confirmation.
 */
import { onMounted, ref } from 'vue'
import { useRouter } from 'vue-router'
import { useToast } from '@nuxt/ui/composables'
import { useI18n } from 'vue-i18n'

import type { HistoryEntry } from '@/api/types'
import DataState from '@/components/DataState.vue'
import SettingsDataResetButton from '@/components/settings/SettingsDataResetButton.vue'
import { translateServerMessage } from '@/i18n/server'
import { HISTORY_KINDS, HISTORY_PERIODS, useHistoryStore } from '@/stores/history'
import { formatBytes, formatMoment } from '@/utils/format'

const { t } = useI18n()
const router = useRouter()
const toast = useToast()
const store = useHistoryStore()
const expanded = ref<Set<number>>(new Set())
const adding = ref<number | null>(null)

function toggle(id: number): void {
  const next = new Set(expanded.value)
  if (next.has(id)) next.delete(id)
  else next.add(id)
  expanded.value = next
}

function failureText(entry: HistoryEntry): string {
  if (!entry.error_code) return ''
  return translateServerMessage({ code: entry.error_code, params: entry.error_params ?? null, message: entry.error_code })
}

async function readd(entry: HistoryEntry): Promise<void> {
  adding.value = entry.id
  const added = await store.readd(entry)
  adding.value = null
  if (!added) return
  toast.add({
    title: t('history.readd.done', { name: entry.name }),
    color: 'success',
    icon: 'i-lucide-list-plus',
    actions: [{ label: t('history.readd.open'), onClick: () => { void router.push('/linkgrabber') } }]
  })
}

onMounted(() => {
  void store.refresh()
})
</script>

<template>
  <UDashboardPanel id="history">
    <template #header>
      <UDashboardNavbar :title="t('history.title')">
        <template #right>
          <UButton
            icon="i-lucide-refresh-cw"
            color="neutral"
            variant="subtle"
            :label="t('common.actions.refresh')"
            :loading="store.fetching"
            @click="store.refresh()"
          />
        </template>
      </UDashboardNavbar>
    </template>

    <template #body>
      <p class="mb-4 text-sm leading-6 text-muted">{{ t('history.intro') }}</p>

      <form class="mb-4 grid gap-3 border border-muted bg-default p-4 md:grid-cols-4" @submit.prevent="store.refresh()">
        <UFormField :label="t('history.filters.search')" class="md:col-span-4">
          <UInput
            v-model="store.filters.search"
            icon="i-lucide-search"
            :placeholder="t('history.filters.search_placeholder')"
            class="w-full"
            data-testid="history-search"
          />
        </UFormField>
        <UFormField :label="t('history.filters.outcome')">
          <USelect
            v-model="store.filters.outcome"
            :items="[
              { value: 'all', label: t('history.filters.any_outcome') },
              { value: 'completed', label: t('history.outcomes.completed') },
              { value: 'failed', label: t('history.outcomes.failed') }
            ]"
            value-key="value"
            class="w-full"
            data-testid="history-outcome"
          />
        </UFormField>
        <UFormField :label="t('history.filters.kind')">
          <USelect
            v-model="store.filters.kind"
            :items="[{ value: 'all', label: t('history.filters.any_kind') }, ...HISTORY_KINDS.map(kind => ({ value: kind, label: t(`history.kinds.${kind}`) }))]"
            value-key="value"
            class="w-full"
            data-testid="history-kind"
          />
        </UFormField>
        <UFormField :label="t('history.filters.period')">
          <USelect
            v-model="store.filters.period"
            :items="[{ value: 'all', label: t('history.filters.any_period') }, ...HISTORY_PERIODS.map(period => ({ value: period, label: t(`history.periods.${period}`) }))]"
            value-key="value"
            class="w-full"
            data-testid="history-period"
          />
        </UFormField>
        <div class="flex flex-wrap items-end gap-2">
          <UButton type="submit" icon="i-lucide-filter" :label="t('history.filters.apply')" :loading="store.fetching" />
          <UButton
            type="button"
            color="neutral"
            variant="ghost"
            icon="i-lucide-x"
            :label="t('history.filters.clear')"
            @click="store.clearFilters(); store.refresh()"
          />
        </div>
      </form>

      <div class="mb-2 flex flex-wrap items-center justify-between gap-2">
        <h2 class="text-sm font-semibold text-highlighted">
          {{ t('history.list.title') }}
          <span v-if="store.settled" class="numeric ml-2 text-xs font-normal text-muted">
            {{ t('history.list.shown', { shown: store.entries.length, total: store.total }) }}
          </span>
        </h2>
        <SettingsDataResetButton target="history" :count="store.stored" @cleared="store.refresh()" />
      </div>

      <DataState :loading="store.loading" :error="store.error" :empty="store.settled && store.entries.length === 0" :rows="6">
        <p class="text-sm text-muted">{{ t('history.list.empty') }}</p>
      </DataState>

      <ul v-if="store.entries.length" class="divide-y divide-muted border border-muted bg-default" data-testid="history-list">
        <li v-for="entry in store.entries" :key="entry.id" class="px-3 py-2" :data-testid="`history-entry-${entry.id}`">
          <div class="flex flex-wrap items-start gap-x-3 gap-y-1">
            <UBadge :color="entry.outcome === 'completed' ? 'success' : 'error'" variant="subtle" size="sm">
              {{ t(`history.outcomes.${entry.outcome}`) }}
            </UBadge>
            <span class="min-w-0 flex-1 break-words text-sm font-medium text-highlighted">{{ entry.name }}</span>
            <span class="shrink-0 text-xs text-muted">{{ t(`history.kinds.${entry.kind}`) }}</span>
            <span class="numeric shrink-0 text-xs text-muted">
              {{ formatBytes(entry.total_bytes) }} · {{ t('history.list.files', { count: entry.file_count }, entry.file_count) }}
            </span>
            <span class="numeric shrink-0 text-xs text-muted">{{ formatMoment(entry.finished_at) }}</span>
            <UButton
              size="xs"
              color="neutral"
              variant="outline"
              icon="i-lucide-list-plus"
              :label="t('history.readd.button')"
              :disabled="entry.sources.length === 0"
              :title="entry.sources.length === 0 ? t('history.readd.no_sources') : undefined"
              :loading="adding === entry.id"
              :data-testid="`history-readd-${entry.id}`"
              @click="readd(entry)"
            />
            <UButton
              size="xs"
              variant="ghost"
              color="neutral"
              :icon="expanded.has(entry.id) ? 'i-lucide-chevron-down' : 'i-lucide-chevron-right'"
              :aria-expanded="expanded.has(entry.id)"
              :aria-label="expanded.has(entry.id) ? t('history.list.collapse') : t('history.list.expand')"
              @click="toggle(entry.id)"
            />
          </div>
          <p v-if="entry.outcome === 'failed' && entry.error_code" class="mt-1 break-words text-xs text-error">
            {{ failureText(entry) }}
          </p>
          <dl v-if="expanded.has(entry.id)" class="mt-2 grid gap-x-4 gap-y-1 pl-1 text-xs">
            <div v-if="entry.category" class="flex gap-2">
              <dt class="shrink-0 font-medium text-muted">{{ t('history.list.category') }}</dt>
              <dd class="min-w-0 break-words text-highlighted">{{ entry.category }}</dd>
            </div>
            <div class="flex gap-2">
              <dt class="shrink-0 font-medium text-muted">{{ t('history.list.destination') }}</dt>
              <dd class="numeric min-w-0 break-all text-highlighted">{{ entry.destination }}</dd>
            </div>
            <div class="flex gap-2">
              <dt class="shrink-0 font-medium text-muted">{{ t('history.list.added') }}</dt>
              <dd class="numeric min-w-0 text-highlighted">{{ formatMoment(entry.created_at) }}</dd>
            </div>
            <div class="flex gap-2">
              <dt class="shrink-0 font-medium text-muted">{{ t('history.list.sources') }}</dt>
              <dd class="min-w-0 text-highlighted">
                <ul v-if="entry.sources.length">
                  <li v-for="source in entry.sources" :key="source" class="numeric break-all">{{ source }}</li>
                </ul>
                <span v-else class="text-muted">{{ t('history.readd.no_sources') }}</span>
              </dd>
            </div>
          </dl>
        </li>
      </ul>

      <div v-if="store.entries.length < store.total" class="mt-3 flex items-center gap-3">
        <UButton
          size="xs"
          color="neutral"
          variant="outline"
          :label="t('history.list.more')"
          :loading="store.fetching"
          data-testid="history-more"
          @click="store.loadMore()"
        />
      </div>
    </template>
  </UDashboardPanel>
</template>
