<script setup lang="ts">
/**
 * Subscriptions: channels, playlists and galleries polled on their own schedule
 * (RD-080-07), and the administrator's own scripts run on a cron schedule (RD-130-19).
 *
 * The editor is `SubscriptionForm` and an unfolded row's archive `SubscriptionArchive`
 * (RD-140-27); the view keeps the list, its row actions and the notice line.
 */
import { onMounted, onUnmounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { api } from '@/api/client'
import type { Category, Subscription, SubscriptionRequest } from '@/api/types'
import { useSubscriptionsStore } from '@/stores/subscriptions'
import DataState from '@/components/DataState.vue'
import FormListLayout from '@/components/FormListLayout.vue'
import SubscriptionArchive from '@/components/SubscriptionArchive.vue'
import SubscriptionForm from '@/components/SubscriptionForm.vue'
import { useConfirm } from '@/composables/useConfirm'
import { duplicateName } from '@/utils/copyName'
import AreaBackupButtons from '@/components/AreaBackupButtons.vue'
import { formatMoment } from '@/utils/format'

/** Matches `MAX_NAME` in `crates/rd-api/src/subscription_handlers.rs`. */
const MAX_SUBSCRIPTION_NAME = 200

const { t } = useI18n()
const store = useSubscriptionsStore()
const confirm = useConfirm()
const categories = ref<Category[]>([])
const editing = ref<string | null>(null)
const subscriptionForm = ref<InstanceType<typeof SubscriptionForm> | null>(null)
const expanded = ref<string | null>(null)

onMounted(async () => {
  // The stream is opened here rather than in `App.vue`: this is the only view that reads
  // subscriptions, so nothing is listening while nobody is looking at them.
  store.connectEvents()
  await Promise.all([
    store.refresh(),
    api.GET('/api/v1/categories').then(response => {
      categories.value = response.data ?? []
    })
  ])
})

onUnmounted(() => {
  store.disconnectEvents()
})

/**
 * What the view says about an action whose result arrives later (RD-106-09).
 *
 * "Check now" answered nothing at all: the request was not awaited, the button showed no
 * state, and the store's error was the only trace a failure left. The notice line is the
 * pattern `design.md` names for this — the view's own statement, not a toast — and it is used
 * three times here: the check was accepted, it could not be started, it finished.
 */
const notice = ref<{ text: string, tone: 'info' | 'error' } | null>(null)

async function checkNow(subscription: Subscription): Promise<void> {
  const result = await store.pollNow(subscription.id)
  // A second press while the first request is in flight is refused without a message: the
  // one already standing is still true, and clearing it would make the button look dead.
  if (!result.ok && result.error === null) return
  notice.value = result.ok
    // Deliberately not "finished": the server answers before the poll has run, and saying
    // otherwise is what made the empty list afterwards look like a check that found nothing.
    ? { text: t('subscriptions.notices.poll_started', { name: subscription.name }), tone: 'info' }
    : {
        text: t('subscriptions.notices.poll_start_failed', {
          name: subscription.name,
          error: result.error ?? ''
        }),
        tone: 'error'
      }
}

// The end of a check reaches the view through the event stream; the list refreshes itself in
// the store, and this turns the same event into the sentence that closes the loop.
watch(() => store.lastPoll, finished => {
  if (!finished) return
  const subscription = store.subscriptions.find(entry => entry.id === finished.subscriptionId)
  const name = subscription?.name ?? ''
  notice.value = finished.error
    ? { text: t('subscriptions.notices.poll_failed', { name, error: finished.error }), tone: 'error' }
    : {
        text: t('subscriptions.notices.poll_finished', {
          name,
          found: finished.found,
          accepted: finished.accepted,
          skipped: finished.skipped
        }),
        tone: 'info'
      }
})

function edit(subscription: Subscription): void {
  subscriptionForm.value?.edit(subscription)
}

function toggleDetails(subscription: Subscription): void {
  expanded.value = expanded.value === subscription.id ? null : subscription.id
}

/// Copies a subscription as the starting point for a similar one.
///
/// Client-side, on the ordinary create endpoint, the way category rules have done it for a
/// while — there is nothing a server-side clone would do better.
///
/// The copy arrives switched off and without a key. The key lives in the vault behind a
/// reference, and handing the copy the same reference would leave two subscriptions quietly
/// sharing one credential, so it is asked for again. Runtime state — what has already been
/// seen, when it last ran, its failure count — belongs to the original and is not copied.
async function duplicate(subscription: Subscription): Promise<void> {
  const body: SubscriptionRequest = {
    name: duplicateName(
      subscription.name,
      store.subscriptions.map(entry => entry.name),
      t('subscriptions.list.copy_suffix'),
      MAX_SUBSCRIPTION_NAME
    ),
    url: subscription.url,
    kind: subscription.kind,
    enabled: false,
    mode: subscription.mode,
    category_id: subscription.category_id ?? null,
    // The response marks these optional; the request does not. Defaulted to what the form
    // itself would send for a subscription that never set them.
    priority: subscription.priority ?? 'normal',
    interval_seconds: subscription.interval_seconds,
    filters: subscription.filters ?? {
      title_contains: [],
      title_excludes: [],
      languages: [],
      min_duration_seconds: null,
      max_duration_seconds: null,
      published_after: null,
      min_height: null
    },
    backlog: subscription.backlog ?? { mode: 'from_now' },
    category_map: [...(subscription.category_map ?? [])],
    source_categories: [...(subscription.source_categories ?? [])],
    schedule: subscription.schedule ?? null,
    api_key: null
  }
  // The new row appearing in the list is the feedback; a failure surfaces through the store's
  // own error, the same way creating one from the form does.
  await store.create(body)
}

/// Deletes a subscription, after asking.
///
/// The design document has required a confirmation for destructive actions all along; this list
/// simply never did it, and deleting a subscription throws away its filters, its category
/// routing and its history of what it has already seen.
async function removeSubscription(subscription: Subscription): Promise<void> {
  const confirmed = await confirm({
    title: t('subscriptions.remove.title'),
    description: t('subscriptions.remove.description', { name: subscription.name }),
    confirmLabel: t('subscriptions.remove.confirm'),
    confirmIcon: 'i-lucide-trash-2',
    destructive: true
  })
  if (!confirmed) return
  await store.remove(subscription.id)
  if (editing.value === subscription.id) subscriptionForm.value?.reset()
}

/**
 * What a subscription row shows beside its name, and what it keeps behind the dots (RD-110-27).
 *
 * Kind and mode are each one idea with one icon, so they are glyphs whose word is their
 * accessible name; the switch is the enabled state itself, because a boolean that takes effect
 * at once is a switch and not a badge beside a switch. Check now stays beside the row — it has
 * to show its own busy state for the length of the request, which a menu item cannot once the
 * menu has closed. Edit, duplicate and delete are deliberate acts that can afford a menu, and
 * they keep their labels there.
 */
const KIND_ICONS: Record<Subscription['kind'], string> = {
  media: 'i-lucide-list-video',
  gallery: 'i-lucide-images',
  feed: 'i-lucide-rss',
  indexer: 'i-lucide-search',
  site_rule: 'i-lucide-file-search',
  script: 'i-lucide-terminal'
}
const MODE_ICONS: Record<Subscription['mode'], string> = {
  review: 'i-lucide-eye',
  auto_queue: 'i-lucide-list-end'
}

function rowActions(subscription: Subscription) {
  return [[
    { label: t('common.actions.edit'), icon: 'i-lucide-pencil', onSelect: () => edit(subscription) },
    { label: t('subscriptions.actions.duplicate'), icon: 'i-lucide-copy', onSelect: () => { void duplicate(subscription) } }
  ], [
    { label: t('common.actions.delete'), icon: 'i-lucide-trash-2', color: 'error' as const, onSelect: () => { void removeSubscription(subscription) } }
  ]]
}
</script>

<template>
  <UDashboardPanel id="subscriptions">
    <template #header>
      <UDashboardNavbar :title="t('subscriptions.title')" />
    </template>
    <template #body>
      <div class="flex flex-col gap-5">
        <UAlert v-if="store.error" color="error" variant="subtle" :description="store.error" />
        <UAlert
          v-if="notice"
          :color="notice.tone === 'error' ? 'error' : 'info'"
          variant="subtle"
          :icon="notice.tone === 'error' ? 'i-lucide-triangle-alert' : 'i-lucide-info'"
          :description="notice.text"
        />

        <FormListLayout>
          <template #form>
            <SubscriptionForm ref="subscriptionForm" v-model:editing="editing" :categories="categories" />
          </template>
          <template #list>
            <section class="border border-muted bg-default p-5">
              <div class="mb-3 flex flex-wrap items-center justify-between gap-2">
                <h2 class="text-sm font-semibold">{{ t('subscriptions.list.title') }}</h2>
                <AreaBackupButtons area="subscriptions" @imported="store.refresh()" />
              </div>
              <DataState :loading="store.loading" :empty="!store.error && !store.subscriptions.length" variant="inline" :rows="3">
                <p class="text-sm text-muted">{{ t('subscriptions.list.empty') }}</p>
              </DataState>
              <ul v-if="store.subscriptions.length" class="flex flex-col divide-y divide-muted">
                <li
                  v-for="subscription in store.subscriptions"
                  :key="subscription.id"
                  :class="editing === subscription.id ? 'border border-primary p-3' : 'py-3'"
                >
                  <div class="flex flex-wrap items-center gap-2">
                    <UButton
                      :icon="expanded === subscription.id ? 'i-lucide-chevron-down' : 'i-lucide-chevron-right'"
                      size="xs"
                      color="neutral"
                      variant="ghost"
                      class="shrink-0"
                      :aria-expanded="expanded === subscription.id"
                      :aria-label="expanded === subscription.id ? t('subscriptions.actions.hide_details') : t('subscriptions.actions.details')"
                      :title="expanded === subscription.id ? t('subscriptions.actions.hide_details') : t('subscriptions.actions.details')"
                      @click="toggleDetails(subscription)"
                    />
                    <!-- The name first, from 200 px up; what does not fit beside it wraps under it. -->
                    <span class="min-w-0 grow shrink basis-[200px] truncate font-medium" :title="subscription.name">{{ subscription.name }}</span>
                    <UBadge v-if="editing === subscription.id" color="primary" variant="subtle">{{ t('common.editing') }}</UBadge>
                    <UBadge color="neutral" variant="outline" size="sm" class="shrink-0" role="img" :icon="KIND_ICONS[subscription.kind]" :aria-label="t(`subscriptions.kinds.${subscription.kind}`)" :title="t(`subscriptions.kinds.${subscription.kind}`)" />
                    <UBadge :color="subscription.mode === 'auto_queue' ? 'warning' : 'neutral'" variant="subtle" size="sm" class="shrink-0" role="img" :icon="MODE_ICONS[subscription.mode]" :aria-label="t(`subscriptions.modes.${subscription.mode}`)" :title="t(`subscriptions.modes.${subscription.mode}`)" />
                    <USwitch
                      :model-value="subscription.enabled"
                      :aria-label="subscription.enabled ? t('subscriptions.actions.disable') : t('subscriptions.actions.enable')"
                      :title="subscription.enabled ? t('subscriptions.actions.disable') : t('subscriptions.actions.enable')"
                      @update:model-value="(value: boolean) => store.setEnabled(subscription.id, value)"
                    />
                    <div data-row-actions class="flex shrink-0 items-center">
                      <UButton
                        icon="i-lucide-refresh-cw"
                        size="xs"
                        color="neutral"
                        variant="ghost"
                        :aria-label="t('subscriptions.actions.poll')"
                        :title="t('subscriptions.actions.poll')"
                        :loading="store.pollingIds.has(subscription.id)"
                        :disabled="store.pollingIds.has(subscription.id)"
                        :data-testid="`subscription-poll-${subscription.id}`"
                        @click="checkNow(subscription)"
                      />
                      <UDropdownMenu :items="rowActions(subscription)" :content="{ align: 'end' }">
                        <UButton icon="i-lucide-ellipsis" size="xs" color="neutral" variant="ghost" :aria-label="t('subscriptions.actions.menu')" :title="t('subscriptions.actions.menu')" />
                      </UDropdownMenu>
                    </div>
                  </div>
                  <p class="truncate text-xs text-muted">{{ subscription.url }}</p>
                  <p v-if="subscription.last_error" class="text-xs text-error">{{ subscription.last_error }}</p>
                  <p v-else-if="subscription.last_run_at" class="text-xs text-muted">
                    {{ t('subscriptions.list.last_run', { at: formatMoment(subscription.last_run_at) }) }}
                  </p>

                  <SubscriptionArchive
                    v-if="expanded === subscription.id"
                    :subscription="subscription"
                    @notice="value => (notice = value)"
                  />
                </li>
              </ul>
            </section>
          </template>
        </FormListLayout>
      </div>
    </template>
  </UDashboardPanel>
</template>
