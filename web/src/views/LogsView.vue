<script setup lang="ts">
/**
 * The structured log viewer and the diagnostic bundle (RD-110-02).
 *
 * Everything shown here was redacted before it was stored; the view only draws. The filter
 * bar sits above the list and the list says when it is a full page, per `design.md`: a page
 * that quietly ends is how "nothing matched" gets mistaken for "nothing was asked for".
 */
import { onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { BundleNote, LogLevel, LogRecord } from '@/api/types'
import { BASE_PATH } from '@/basePath'
import DataState from '@/components/DataState.vue'
import SectionHeader from '@/components/SectionHeader.vue'
import { useLogsStore } from '@/stores/logs'
import { formatMoment } from '@/utils/format'

const { t, te } = useI18n()
const store = useLogsStore()
const expanded = ref<Set<number>>(new Set())

const LEVELS: LogLevel[] = ['trace', 'debug', 'info', 'warn', 'error']

const levelItems = [
  { value: '', label: t('logs.levels.all') },
  ...LEVELS.map(level => ({ value: level, label: t(`logs.levels.${level}`) }))
]

/**
 * A line the server sent about the bundle, in the reader's language (RD-120-15).
 *
 * The server writes no prose: an entry's description, its redaction notes and the
 * never-included lines travel as a stable code plus the data the sentence names. A code no
 * catalogue knows falls back to the English text that came with it, never to the raw key --
 * an untranslated sentence is still an answer, a dotted identifier is not.
 */
function noteText(note: BundleNote): string {
  const key = `logs.${note.code}`
  return te(key) ? t(key, { ...(note.params ?? {}) }) : note.text
}

function levelColor(level: LogLevel): 'error' | 'warning' | 'primary' | 'neutral' {
  if (level === 'error') return 'error'
  if (level === 'warn') return 'warning'
  if (level === 'info') return 'primary'
  return 'neutral'
}

function hasFields(record: LogRecord): boolean {
  return Object.keys(record.fields).length > 0
}

function toggle(id: number): void {
  const next = new Set(expanded.value)
  if (next.has(id)) next.delete(id)
  else next.add(id)
  expanded.value = next
}

function downloadHref(fileName: string): string {
  return `${BASE_PATH}/api/v1/diagnostics/bundles/${encodeURIComponent(fileName)}`
}

function toggleEntry(id: string, checked: boolean): void {
  const rest = store.selected.filter(entry => entry !== id)
  store.selected = checked ? [...rest, id] : rest
}

onMounted(() => {
  void store.refresh()
})
</script>

<template>
  <UDashboardPanel id="logs">
    <template #header>
      <UDashboardNavbar :title="t('logs.title')">
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
      <p class="mb-4 text-sm leading-6 text-muted">{{ t('logs.intro') }}</p>

      <form class="mb-4 grid gap-3 border border-muted bg-default p-4 md:grid-cols-6" @submit.prevent="store.refresh()">
        <UFormField :label="t('logs.filters.level')">
          <USelect v-model="store.filters.level" :items="levelItems" value-key="value" class="w-full" data-testid="log-level" />
        </UFormField>
        <UFormField :label="t('logs.filters.component')">
          <UInput v-model="store.filters.component" :placeholder="t('logs.filters.component_placeholder')" class="w-full" data-testid="log-component" />
        </UFormField>
        <UFormField :label="t('logs.filters.code')">
          <UInput v-model="store.filters.code" class="w-full" data-testid="log-code" />
        </UFormField>
        <UFormField :label="t('logs.filters.correlation')">
          <UInput v-model="store.filters.correlationId" class="w-full" data-testid="log-correlation" />
        </UFormField>
        <UFormField :label="t('logs.filters.search')" class="md:col-span-2">
          <UInput v-model="store.filters.search" icon="i-lucide-search" class="w-full" data-testid="log-search" />
        </UFormField>
        <div class="flex flex-wrap gap-2 md:col-span-6">
          <UButton type="submit" icon="i-lucide-filter" :label="t('logs.filters.apply')" :loading="store.fetching" />
          <UButton
            type="button"
            color="neutral"
            variant="ghost"
            icon="i-lucide-x"
            :label="t('logs.filters.clear')"
            @click="store.clearFilters(); store.refresh()"
          />
        </div>
      </form>

      <UAlert
        v-if="store.dropped > 0"
        class="mb-4"
        color="warning"
        variant="subtle"
        :description="t('logs.list.dropped', { count: store.dropped })"
      />

      <div class="mb-2 flex flex-wrap items-baseline justify-between gap-2">
        <h2 class="text-sm font-semibold text-highlighted">
          {{ t('logs.list.title') }}
          <span v-if="store.settled" class="numeric ml-2 text-xs font-normal text-muted">
            {{ t('logs.list.shown', { shown: store.records.length, total: store.total }) }}
          </span>
        </h2>
        <p v-if="store.retention" class="text-xs text-muted">
          {{ t('logs.list.retention', { days: store.retention.days, records: store.retention.records }) }}
        </p>
      </div>

      <DataState :loading="store.loading" :error="store.error" :empty="store.settled && store.records.length === 0" :rows="6">
        <p class="text-sm text-muted">{{ t('logs.list.empty') }}</p>
      </DataState>

      <ul v-if="store.records.length" class="divide-y divide-muted border border-muted bg-default" data-testid="log-list">
        <li v-for="record in store.records" :key="record.id" class="px-3 py-2">
          <div class="flex flex-wrap items-start gap-x-3 gap-y-1">
            <span class="numeric shrink-0 text-xs text-muted">{{ formatMoment(record.recorded_at) }}</span>
            <UBadge :color="levelColor(record.level)" variant="subtle" size="sm">{{ t(`logs.levels.${record.level}`) }}</UBadge>
            <span class="numeric shrink-0 text-xs text-muted">{{ record.component }}</span>
            <UBadge v-if="record.code" color="neutral" variant="outline" size="sm" class="numeric">{{ record.code }}</UBadge>
            <UBadge v-if="record.correlation_id" color="neutral" variant="soft" size="sm" class="numeric" :title="t('logs.filters.correlation')">{{ record.correlation_id }}</UBadge>
            <span class="min-w-0 flex-1 break-words text-sm text-highlighted">{{ record.message }}</span>
            <UButton
              v-if="hasFields(record)"
              size="xs"
              variant="ghost"
              color="neutral"
              :icon="expanded.has(record.id) ? 'i-lucide-chevron-down' : 'i-lucide-chevron-right'"
              :aria-expanded="expanded.has(record.id)"
              :aria-label="expanded.has(record.id) ? t('logs.list.collapse') : t('logs.list.expand')"
              :title="expanded.has(record.id) ? t('logs.list.collapse') : t('logs.list.expand')"
              @click="toggle(record.id)"
            />
          </div>
          <dl v-if="expanded.has(record.id)" class="mt-2 grid gap-x-4 gap-y-1 text-xs sm:grid-cols-[auto_1fr]" data-testid="log-fields">
            <template v-for="(value, name) in record.fields" :key="name">
              <dt class="numeric text-muted">{{ name }}</dt>
              <dd class="numeric break-all text-highlighted">{{ value }}</dd>
            </template>
          </dl>
        </li>
      </ul>

      <div v-if="store.fullPage" class="mt-3 flex flex-wrap items-center gap-3">
        <p class="text-xs text-muted">{{ t('logs.list.full_page') }}</p>
        <UButton
          size="xs"
          color="neutral"
          variant="outline"
          icon="i-lucide-history"
          :label="t('logs.list.older')"
          :loading="store.fetching"
          @click="store.loadOlder()"
        />
      </div>

      <section class="mt-8 border border-muted bg-default p-5" data-testid="diagnostic-bundle">
        <SectionHeader :eyebrow="t('logs.bundle.eyebrow')" :title="t('logs.bundle.title')" :description="t('logs.bundle.intro')" />
        <div class="mt-4 flex flex-wrap gap-2">
          <UButton
            color="neutral"
            variant="outline"
            icon="i-lucide-list-checks"
            :label="t('logs.bundle.preview')"
            :loading="store.bundleBusy"
            @click="store.loadPreview()"
          />
          <UButton
            icon="i-lucide-package"
            :label="t('logs.bundle.create')"
            :disabled="!store.canCreate"
            :loading="store.bundleBusy && store.preview !== null"
            @click="store.create()"
          />
        </div>
        <UAlert v-if="store.bundleError" class="mt-4" color="error" variant="subtle" :description="store.bundleError" />

        <div v-if="store.preview" class="mt-4 space-y-4" data-testid="bundle-preview">
          <p class="text-xs text-muted">{{ t('logs.bundle.directory', { directory: store.preview.directory }) }}</p>
          <div>
            <h3 class="text-sm font-semibold text-highlighted">{{ t('logs.bundle.entries') }}</h3>
            <p class="mb-2 text-xs text-muted">{{ t('logs.bundle.select_hint') }}</p>
            <ul class="divide-y divide-muted border border-muted">
              <li v-for="entry in store.preview.entries" :key="entry.id" class="px-3 py-2">
                <UCheckbox
                  :model-value="store.selected.includes(entry.id)"
                  :label="entry.path"
                  @update:model-value="(value: boolean | 'indeterminate') => toggleEntry(entry.id, value === true)"
                />
                <p class="mt-1 text-xs text-muted">
                  {{ noteText(entry.description) }} · {{ t('logs.bundle.items', { count: entry.items }) }}
                </p>
                <p v-if="entry.redactions.length" class="mt-1 text-xs text-muted">
                  <span class="font-medium">{{ t('logs.bundle.redactions') }}:</span> {{ entry.redactions.map(noteText).join('; ') }}
                </p>
              </li>
            </ul>
          </div>
          <div>
            <h3 class="text-sm font-semibold text-highlighted">{{ t('logs.bundle.excluded') }}</h3>
            <ul class="mt-1 list-disc pl-5 text-xs text-muted">
              <li v-for="line in store.preview.excluded" :key="line.code">{{ noteText(line) }}</li>
            </ul>
          </div>
        </div>

        <div v-if="store.created" class="mt-4 border border-muted bg-elevated p-4 text-sm" data-testid="bundle-created">
          <p class="text-highlighted">{{ t('logs.bundle.created', { file: store.created.file_name }) }}</p>
          <p class="numeric mt-1 text-xs text-muted">{{ store.created.path }} · {{ t('logs.bundle.bytes', { bytes: store.created.bytes }) }}</p>
          <UButton
            class="mt-3"
            size="xs"
            color="neutral"
            variant="outline"
            icon="i-lucide-download"
            :label="t('logs.bundle.download')"
            :to="downloadHref(store.created.file_name)"
            external
            download
          />
        </div>
      </section>
    </template>
  </UDashboardPanel>
</template>
