<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import {
  dedupeDownload,
  getDownloadDuplicates,
  getPackageCollisionPolicy,
  setPackageCollisionPolicy,
  type CollisionPolicy,
  type ContentDuplicate,
  type DuplicateReport,
  type PackageCollisionPolicy
} from '@/api/storage'
import { translateServerMessage } from '@/i18n/server'
import { formatBytes } from '@/utils/format'
import CollisionPolicySelect from '@/components/storage/CollisionPolicySelect.vue'
import FormFeedback from '@/components/FormFeedback.vue'
import SearchableSelect from '@/components/SearchableSelect.vue'

/**
 * A package's collision policy and its files' duplicates (RD-150-01, RD-150-02).
 *
 * Source and content duplicates are two lists on purpose: "already queued" and "these bytes are
 * already on disk" ask for different decisions. A content duplicate of a finished file can be
 * replaced by a link to the original — only on request, and only after the service hashed both.
 */
const props = defineProps<{
  packageId: string
  packageName: string
  downloads: { id: string, file_name: string, state: string }[]
}>()
const emit = defineEmits<{ close: [changed: boolean] }>()
const { t } = useI18n()

const view = ref<PackageCollisionPolicy | null>(null)
const policy = ref<CollisionPolicy | null>(null)
const saving = ref(false)
const error = ref<string | null>(null)
const success = ref<string | null>(null)
const changed = ref(false)

const selectedDownload = ref<string>(props.downloads[0]?.id ?? '')
const report = ref<DuplicateReport | null>(null)
const reportLoading = ref(false)
const linking = ref<string | null>(null)

const downloadItems = computed(() => props.downloads.map(item => ({ label: item.file_name, value: item.id })))
const selectedFinished = computed(() => props.downloads.find(item => item.id === selectedDownload.value)?.state === 'completed')

/** What the package inherits when it has no policy of its own, and from where. */
const inheritLabel = computed(() => {
  if (!view.value) return t('downloads.collision.inherit')
  const inherited = view.value.category ?? view.value.global
  const from = view.value.category ? 'category' : 'global'
  return t('downloads.collision.inherit_from', {
    policy: t(`downloads.collision.policies.${inherited}`),
    level: t(`downloads.collision.levels.${from}`)
  })
})

async function loadPolicy(): Promise<void> {
  const answer = await getPackageCollisionPolicy(props.packageId)
  if (!answer.ok) {
    error.value = translateServerMessage(answer.message)
    return
  }
  view.value = answer.data
  policy.value = answer.data.own ?? null
}

async function savePolicy(): Promise<void> {
  saving.value = true
  error.value = null
  success.value = null
  const answer = await setPackageCollisionPolicy(props.packageId, policy.value)
  saving.value = false
  if (!answer.ok) {
    error.value = translateServerMessage(answer.message)
    return
  }
  view.value = answer.data
  changed.value = true
  success.value = t('downloads.collision.saved', {
    policy: t(`downloads.collision.policies.${answer.data.effective.policy}`)
  })
}

async function loadReport(): Promise<void> {
  report.value = null
  if (!selectedDownload.value) return
  reportLoading.value = true
  const answer = await getDownloadDuplicates(selectedDownload.value)
  reportLoading.value = false
  if (!answer.ok) {
    error.value = translateServerMessage(answer.message)
    return
  }
  report.value = answer.data
}

function canLink(entry: ContentDuplicate): boolean {
  return selectedFinished.value && report.value?.content_basis === 'verified_hash'
    && !entry.missing && entry.same_file_system !== false
}

async function link(entry: ContentDuplicate): Promise<void> {
  linking.value = entry.download_id
  error.value = null
  success.value = null
  const answer = await dedupeDownload(selectedDownload.value, entry.download_id)
  linking.value = null
  if (!answer.ok) {
    error.value = translateServerMessage(answer.message)
    return
  }
  changed.value = true
  success.value = t('downloads.duplicates.linked', { size: formatBytes(String(answer.data.freed_bytes)) })
  await loadReport()
}

watch(selectedDownload, () => void loadReport())
onMounted(() => {
  void loadPolicy()
  void loadReport()
})
</script>

<template>
  <UModal
    :title="t('downloads.storage_modal.title', { name: props.packageName })"
    :description="t('downloads.storage_modal.description')"
    :close="{ onClick: () => emit('close', changed) }"
  >
    <template #body>
      <div class="space-y-5">
        <FormFeedback :error="error" :message="success" />

        <form class="space-y-3" data-testid="package-collision-form" @submit.prevent="savePolicy">
          <UFormField :label="t('downloads.collision.label')" :description="t('downloads.collision.package_description')">
            <CollisionPolicySelect v-model="policy" :inherit-label="inheritLabel" :disabled="!view" />
          </UFormField>
          <p v-if="view" class="text-xs text-muted" data-testid="collision-effective">
            {{ t('downloads.collision.effective', {
              policy: t(`downloads.collision.policies.${view.effective.policy}`),
              level: t(`downloads.collision.levels.${view.effective.source}`)
            }) }}
          </p>
          <div class="flex justify-start gap-2">
            <UButton type="submit" icon="i-lucide-save" :label="t('common.actions.save')" :loading="saving" :disabled="!view" />
          </div>
        </form>

        <USeparator />
        <section class="space-y-3" data-testid="duplicates">
          <UFormField :label="t('downloads.duplicates.file')">
            <SearchableSelect v-model="selectedDownload" :items="downloadItems" class="w-full" />
          </UFormField>
          <p v-if="reportLoading" class="text-xs text-muted">{{ t('downloads.duplicates.loading') }}</p>
          <template v-else-if="report">
            <div>
              <p class="text-sm font-medium text-highlighted">{{ t('downloads.duplicates.source_title') }}</p>
              <p class="mt-1 text-xs leading-5 text-muted">{{ t('downloads.duplicates.source_description', { kind: t(`downloads.duplicates.identity.${report.identity.kind}`) }) }}</p>
              <p v-if="!report.source.length" class="mt-2 text-xs text-muted">{{ t('downloads.duplicates.none') }}</p>
              <ul v-else class="mt-2 space-y-1 text-xs" data-testid="source-duplicates">
                <li v-for="(entry, index) in report.source" :key="`${entry.download_id ?? entry.candidate_id}-${index}`" class="flex items-center gap-2">
                  <UBadge :color="entry.location === 'queue' ? 'info' : 'neutral'" variant="subtle" size="sm" :label="t(`downloads.duplicates.location.${entry.location}`)" />
                  <span class="truncate">{{ entry.package_name ? `${entry.package_name} / ` : '' }}{{ entry.file_name ?? '—' }}</span>
                </li>
              </ul>
            </div>
            <div>
              <p class="text-sm font-medium text-highlighted">{{ t('downloads.duplicates.content_title') }}</p>
              <p class="mt-1 text-xs leading-5 text-muted">
                {{ report.content_basis ? t(`downloads.duplicates.basis.${report.content_basis}`) : t('downloads.duplicates.basis.none') }}
              </p>
              <p v-if="report.content_basis && !report.content.length" class="mt-2 text-xs text-muted">{{ t('downloads.duplicates.none') }}</p>
              <ul v-else-if="report.content.length" class="mt-2 space-y-2 text-xs" data-testid="content-duplicates">
                <li v-for="entry in report.content" :key="entry.download_id" class="flex flex-wrap items-center gap-2">
                  <span class="min-w-0 flex-1 truncate font-mono" :title="entry.path">{{ entry.path }}</span>
                  <span class="text-muted">{{ formatBytes(String(entry.size_bytes)) }}</span>
                  <UBadge v-if="entry.missing" color="warning" variant="subtle" size="sm" :label="t('downloads.duplicates.missing')" />
                  <UButton
                    v-if="canLink(entry)"
                    size="xs"
                    variant="outline"
                    icon="i-lucide-link"
                    :label="t('downloads.duplicates.link')"
                    :title="t('downloads.duplicates.link_title')"
                    :loading="linking === entry.download_id"
                    :disabled="linking !== null"
                    @click="link(entry)"
                  />
                  <span v-else-if="entry.same_file_system === false" class="text-muted">{{ t('downloads.duplicates.other_file_system') }}</span>
                </li>
              </ul>
            </div>
          </template>
        </section>
      </div>
    </template>
  </UModal>
</template>
