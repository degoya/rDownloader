<script setup lang="ts">
/**
 * The append-only audit log (RD-110-03).
 *
 * A page of its own rather than a tab of the log viewer: the two lists answer different
 * questions ("what went wrong" against "who did what"), they are kept for different lengths
 * of time, and this one is a security surface that should not be one click deep inside a
 * diagnostics page. Row conventions follow `design.md`: filters above the list, "full page"
 * said out loud, details behind the chevron pair.
 */
import { onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { AuditOutcome, AuditRecord } from '@/api/types'
import DataState from '@/components/DataState.vue'
import SectionHeader from '@/components/SectionHeader.vue'
import { useAuditStore } from '@/stores/audit'
import { formatMoment } from '@/utils/format'

const { t } = useI18n()
const store = useAuditStore()
const expanded = ref<Set<number>>(new Set())

const ACTOR_KINDS = ['session', 'token', 'anonymous', 'system'] as const

function outcomeColor(outcome: AuditOutcome): 'success' | 'error' {
  return outcome === 'success' ? 'success' : 'error'
}

function hasDetails(record: AuditRecord): boolean {
  return Object.keys(record.details).length > 0
}

function toggle(id: number): void {
  const next = new Set(expanded.value)
  if (next.has(id)) next.delete(id)
  else next.add(id)
  expanded.value = next
}

function actionLabel(action: string): string {
  return t(`audit.actions.${action}`)
}

function targetOf(record: AuditRecord): string {
  if (!record.target_kind) return ''
  const name = record.target_name ?? record.target_id ?? ''
  return name ? `${record.target_kind} · ${name}` : record.target_kind
}

onMounted(() => {
  void store.refresh()
})
</script>

<template>
  <UDashboardPanel id="audit">
    <template #header>
      <UDashboardNavbar :title="t('audit.title')">
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
      <p class="mb-4 text-sm leading-6 text-muted">{{ t('audit.intro') }}</p>

      <form class="mb-4 grid gap-3 border border-muted bg-default p-4 md:grid-cols-6" @submit.prevent="store.refresh()">
        <UFormField :label="t('audit.filters.action')">
          <USelect
            v-model="store.filters.action"
            :items="[{ value: '', label: t('audit.filters.any_action') }, ...store.actions.map(action => ({ value: action, label: actionLabel(action) }))]"
            value-key="value"
            class="w-full"
            data-testid="audit-action"
          />
        </UFormField>
        <UFormField :label="t('audit.filters.outcome')">
          <USelect
            v-model="store.filters.outcome"
            :items="[
              { value: '', label: t('audit.filters.any_outcome') },
              { value: 'success', label: t('audit.outcomes.success') },
              { value: 'failure', label: t('audit.outcomes.failure') }
            ]"
            value-key="value"
            class="w-full"
            data-testid="audit-outcome"
          />
        </UFormField>
        <UFormField :label="t('audit.filters.actor_kind')">
          <USelect
            v-model="store.filters.actorKind"
            :items="[{ value: '', label: t('audit.filters.any_actor') }, ...ACTOR_KINDS.map(kind => ({ value: kind, label: t(`audit.actors.${kind}`) }))]"
            value-key="value"
            class="w-full"
            data-testid="audit-actor-kind"
          />
        </UFormField>
        <UFormField :label="t('audit.filters.target_kind')">
          <UInput v-model="store.filters.targetKind" :placeholder="t('audit.filters.target_placeholder')" class="w-full" data-testid="audit-target-kind" />
        </UFormField>
        <UFormField :label="t('audit.filters.target_id')">
          <UInput v-model="store.filters.targetId" class="w-full" data-testid="audit-target-id" />
        </UFormField>
        <UFormField :label="t('audit.filters.trace')">
          <UInput v-model="store.filters.traceId" class="w-full" data-testid="audit-trace" />
        </UFormField>
        <div class="flex flex-wrap gap-2 md:col-span-6">
          <UButton type="submit" icon="i-lucide-filter" :label="t('audit.filters.apply')" :loading="store.fetching" />
          <UButton
            type="button"
            color="neutral"
            variant="ghost"
            icon="i-lucide-x"
            :label="t('audit.filters.clear')"
            @click="store.clearFilters(); store.refresh()"
          />
          <!-- Beside the filters rather than in the navbar: the file is the filter, and the
               two controls belong where the reader decided what it contains. -->
          <UButton
            class="ml-auto"
            color="neutral"
            variant="outline"
            icon="i-lucide-download"
            :label="t('audit.export')"
            :href="store.exportHref"
            :to="store.exportHref"
            external
            download
            data-testid="audit-export"
          />
        </div>
      </form>

      <div class="mb-2 flex flex-wrap items-baseline justify-between gap-2">
        <h2 class="text-sm font-semibold text-highlighted">
          {{ t('audit.list.title') }}
          <span v-if="store.settled" class="numeric ml-2 text-xs font-normal text-muted">
            {{ t('audit.list.shown', { shown: store.records.length, total: store.total }) }}
          </span>
        </h2>
        <p v-if="store.retention" class="text-xs text-muted">
          {{ t('audit.list.retention', { days: store.retention.days, records: store.retention.records }) }}
        </p>
      </div>

      <DataState :loading="store.loading" :error="store.error" :empty="store.settled && store.records.length === 0" :rows="6">
        <p class="text-sm text-muted">{{ t('audit.list.empty') }}</p>
      </DataState>

      <ul v-if="store.records.length" class="divide-y divide-muted border border-muted bg-default" data-testid="audit-list">
        <li v-for="record in store.records" :key="record.id" class="px-3 py-2">
          <div class="flex flex-wrap items-start gap-x-3 gap-y-1">
            <span class="numeric shrink-0 text-xs text-muted">{{ formatMoment(record.recorded_at) }}</span>
            <UBadge :color="outcomeColor(record.outcome)" variant="subtle" size="sm">{{ t(`audit.outcomes.${record.outcome}`) }}</UBadge>
            <span class="min-w-0 flex-1 break-words text-sm text-highlighted">{{ actionLabel(record.action) }}</span>
            <span class="shrink-0 text-xs text-muted">
              {{ t(`audit.actors.${record.actor_kind}`) }}<template v-if="record.actor_label"> · {{ record.actor_label }}</template>
            </span>
            <span v-if="targetOf(record)" class="numeric shrink-0 text-xs text-muted">{{ targetOf(record) }}</span>
            <span v-if="record.client_address" class="numeric shrink-0 text-xs text-muted">{{ record.client_address }}</span>
            <UButton
              v-if="hasDetails(record) || record.trace_id"
              size="xs"
              variant="ghost"
              color="neutral"
              :icon="expanded.has(record.id) ? 'i-lucide-chevron-down' : 'i-lucide-chevron-right'"
              :aria-expanded="expanded.has(record.id)"
              :aria-label="expanded.has(record.id) ? t('audit.list.collapse') : t('audit.list.expand')"
              @click="toggle(record.id)"
            />
          </div>
          <dl v-if="expanded.has(record.id)" class="mt-2 grid gap-x-4 gap-y-1 pl-1 text-xs sm:grid-cols-2">
            <div v-if="record.trace_id" class="flex gap-2">
              <dt class="shrink-0 font-medium text-muted">{{ t('audit.filters.trace') }}</dt>
              <dd class="numeric min-w-0 break-all text-highlighted">{{ record.trace_id }}</dd>
            </div>
            <div v-if="record.actor_id" class="flex gap-2">
              <dt class="shrink-0 font-medium text-muted">{{ t('audit.list.actor_id') }}</dt>
              <dd class="numeric min-w-0 break-all text-highlighted">{{ record.actor_id }}</dd>
            </div>
            <div v-for="(value, key) in record.details" :key="key" class="flex gap-2">
              <dt class="shrink-0 font-medium text-muted">{{ key }}</dt>
              <dd class="min-w-0 break-words text-highlighted">{{ value }}</dd>
            </div>
          </dl>
        </li>
      </ul>

      <div v-if="store.fullPage" class="mt-3 flex items-center gap-3">
        <p class="text-xs text-muted">{{ t('audit.list.full_page') }}</p>
        <UButton size="xs" color="neutral" variant="outline" :label="t('audit.list.older')" :loading="store.fetching" @click="store.loadOlder()" />
      </div>

      <section class="mt-8 border border-muted bg-default p-5">
        <SectionHeader :eyebrow="t('audit.append_only.eyebrow')" :title="t('audit.append_only.title')" :description="t('audit.append_only.description')" />
      </section>
    </template>
  </UDashboardPanel>
</template>
