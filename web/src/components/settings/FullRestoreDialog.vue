<script setup lang="ts">
/**
 * Restoring a full backup (RD-160-03), in the order it has to happen: pick the archive, type
 * its passphrase, see what it holds, say where its storage roots lie here, test it in a
 * throwaway copy, and only then confirm. The restore is staged; the next start switches.
 *
 * The passphrase is asked for every time — the key the schedule keeps is never used for a
 * restore (owner's decision, 2026-09-28) — and cleared when the dialog closes. A test result
 * belongs to exactly the archive, passphrase and mappings it was made with: changing any of
 * them drops it, and the restore stays unavailable until a new test passed.
 */
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import {
  previewRestore,
  startRestore,
  testRestore,
  uploadArchive,
  type RestoreMapping
} from '@/api/fullRestore'
import type { BackupRun, RestorePreview, RestoreProblem, RestoreReport, RestoreSource } from '@/api/types'
import { translateServerMessage } from '@/i18n/server'
import { formatBytes, formatMoment } from '@/utils/format'

const props = defineProps<{ runs: BackupRun[] }>()
const open = defineModel<boolean>('open', { required: true })
const emit = defineEmits<{ staged: [] }>()

const { t } = useI18n()

type SourceKind = 'history' | 'upload' | 'path'

const kind = ref<SourceKind>('history')
const runId = ref<string | undefined>(undefined)
const path = ref('')
const uploadId = ref<string | null>(null)
const uploadName = ref('')
const uploadShare = ref<number | null>(null)
const passphrase = ref('')
const preview = ref<RestorePreview | null>(null)
const targets = ref<Record<string, string>>({})
const report = ref<RestoreReport | null>(null)
const confirmed = ref(false)
const busy = ref<'upload' | 'preview' | 'test' | 'restore' | null>(null)
const error = ref<string | null>(null)
const kindItems = computed(() => [
  { label: t('system.backup.full_restore.source.history'), value: 'history' },
  { label: t('system.backup.full_restore.source.upload'), value: 'upload' },
  { label: t('system.backup.full_restore.source.path'), value: 'path' }
])
const runItems = computed(() => props.runs
  .filter(run => run.state === 'succeeded' && run.archive_name)
  .map(run => ({ label: `${formatMoment(run.started_at)} · ${run.archive_name}`, value: run.id })))

const source = computed<RestoreSource | null>(() => {
  if (kind.value === 'history') return runId.value ? { run_id: runId.value } : null
  if (kind.value === 'upload') return uploadId.value ? { upload_id: uploadId.value } : null
  return path.value.trim() ? { path: path.value.trim() } : null
})

const mappings = computed<RestoreMapping[]>(() => (preview.value?.storage_roots ?? [])
  .map(root => ({ storage_root_id: root.id, path: (targets.value[root.id] ?? '').trim(), from: root.path }))
  .filter(mapping => mapping.path !== '' && mapping.path !== mapping.from)
  .map(({ storage_root_id, path }) => ({ storage_root_id, path })))

const canPreview = computed(() => source.value !== null && passphrase.value.length > 0 && busy.value === null)
const canRestore = computed(() => report.value?.ok === true && confirmed.value && busy.value === null)

// A test result speaks for exactly what it was made with.
watch([source, passphrase, mappings], () => {
  report.value = null
  confirmed.value = false
}, { deep: true })
watch([source, passphrase], () => {
  preview.value = null
})
watch(open, (value) => {
  if (value) return
  passphrase.value = ''
  preview.value = null
  report.value = null
  confirmed.value = false
  error.value = null
})

async function upload(file: File | null | undefined): Promise<void> {
  if (!file) return
  error.value = null
  uploadId.value = null
  uploadName.value = file.name
  busy.value = 'upload'
  const answer = await uploadArchive(file, (share) => {
    uploadShare.value = Math.round(share * 100)
  })
  busy.value = null
  uploadShare.value = null
  if (!answer.ok) {
    error.value = answer.error
    return
  }
  uploadId.value = answer.data
}

async function showPreview(): Promise<void> {
  if (!source.value) return
  error.value = null
  busy.value = 'preview'
  const answer = await previewRestore(source.value, passphrase.value)
  busy.value = null
  if (!answer.ok) {
    error.value = answer.error
    return
  }
  preview.value = answer.data
  targets.value = Object.fromEntries(answer.data.storage_roots.map(root => [root.id, root.native ? root.path : '']))
}

async function runTest(): Promise<void> {
  if (!source.value) return
  error.value = null
  busy.value = 'test'
  const answer = await testRestore(source.value, passphrase.value, mappings.value)
  busy.value = null
  if (!answer.ok) {
    error.value = answer.error
    return
  }
  report.value = answer.data
}

async function restore(): Promise<void> {
  if (!source.value || !canRestore.value) return
  error.value = null
  busy.value = 'restore'
  const answer = await startRestore(source.value, passphrase.value, mappings.value)
  busy.value = null
  if (!answer.ok) {
    error.value = answer.error
    return
  }
  open.value = false
  emit('staged')
}

function problemText(problem: RestoreProblem): string {
  return translateServerMessage({
    code: problem.code,
    message: problem.code,
    params: { count: String(problem.count) }
  })
}

function kindLabel(value: string): string {
  return t(`system.backup.full_restore.preview.kind.${value}`)
}
</script>

<template>
  <UModal
    v-model:open="open"
    :title="t('system.backup.full_restore.dialog.title')"
    :description="t('system.backup.full_restore.dialog.description')"
    :ui="{ content: 'max-w-3xl', footer: 'justify-end' }"
  >
    <template #body>
      <div class="space-y-6" data-testid="full-restore-dialog">
        <UAlert v-if="error" color="error" variant="subtle" icon="i-lucide-circle-alert" :description="error" />

        <section class="space-y-3">
          <h3 class="text-sm font-semibold text-highlighted">{{ t('system.backup.full_restore.source.title') }}</h3>
          <URadioGroup v-model="kind" orientation="horizontal" :items="kindItems" data-testid="full-restore-kind" />
          <template v-if="kind === 'history'">
            <p v-if="!runItems.length" class="text-sm text-muted">{{ t('system.backup.full_restore.source.no_runs') }}</p>
            <USelect
              v-else
              v-model="runId"
              :items="runItems"
              value-key="value"
              :placeholder="t('system.backup.full_restore.source.run_placeholder')"
              class="w-full"
              data-testid="full-restore-run"
            />
          </template>
          <div v-else-if="kind === 'upload'" class="flex flex-wrap items-center gap-3">
            <UFileUpload
              v-slot="{ open }"
              :model-value="null"
              accept=".rdbackup"
              reset
              :dropzone="false"
              data-testid="full-restore-file"
              @update:model-value="upload"
            >
              <UButton
                icon="i-lucide-upload"
                color="neutral"
                variant="outline"
                :label="t('system.backup.full_restore.source.file_pick')"
                :loading="busy === 'upload'"
                @click="open()"
              />
            </UFileUpload>
            <span v-if="uploadShare !== null" class="text-sm text-muted">
              {{ t('system.backup.full_restore.source.uploading', { percent: uploadShare }) }}
            </span>
            <span v-else-if="uploadId" class="text-sm text-toned">
              {{ t('system.backup.full_restore.source.uploaded', { name: uploadName }) }}
            </span>
          </div>
          <UInput
            v-else
            v-model="path"
            class="w-full font-mono"
            :placeholder="t('system.backup.full_restore.source.path_placeholder')"
            data-testid="full-restore-path"
          />
        </section>

        <form class="space-y-3" @submit.prevent="showPreview">
          <UFormField
            name="full-restore-passphrase"
            :label="t('system.backup.full_restore.passphrase.label')"
            :description="t('system.backup.full_restore.passphrase.hint')"
            required
          >
            <UInput v-model="passphrase" type="password" autocomplete="off" class="w-full" data-testid="full-restore-passphrase" />
          </UFormField>
          <UButton
            type="submit"
            icon="i-lucide-scan-search"
            :label="t('system.backup.full_restore.preview.action')"
            :disabled="!canPreview"
            :loading="busy === 'preview'"
            data-testid="full-restore-preview"
          />
        </form>

        <section v-if="preview" class="space-y-3" data-testid="full-restore-contents">
          <h3 class="text-sm font-semibold text-highlighted">{{ t('system.backup.full_restore.preview.title') }}</h3>
          <p class="text-sm text-toned">
            {{ t('system.backup.full_restore.preview.created', { date: formatMoment(preview.created_at), version: preview.app_version, size: formatBytes(String(preview.archive_size)) }) }}
          </p>
          <UAlert v-if="preview.from_newer_version" color="warning" variant="subtle" :description="t('system.backup.full_restore.preview.newer', { version: preview.current_version })" />
          <ul class="grid gap-1 text-sm text-toned sm:grid-cols-2">
            <li v-for="part in preview.parts" :key="part.kind">
              {{ kindLabel(part.kind) }}: {{ t('system.backup.full_restore.preview.part', { count: part.count, size: formatBytes(String(part.size)) }) }}
            </li>
          </ul>
          <p class="text-sm text-toned">
            {{ t('system.backup.full_restore.preview.counts', { categories: preview.categories, accounts: preview.accounts, proxies: preview.proxy_profiles, servers: preview.usenet_servers, subscriptions: preview.subscriptions, hotfolders: preview.hotfolders }) }}
          </p>
          <p class="text-sm text-toned">
            {{ t('system.backup.full_restore.preview.state', { partial: preview.partial_transfers, trust: preview.plugin_trust_rows }) }}
            {{ preview.credentials_included ? t('system.backup.full_restore.preview.credentials_included') : t('system.backup.full_restore.preview.credentials_missing') }}
          </p>

          <h3 class="pt-2 text-sm font-semibold text-highlighted">{{ t('system.backup.full_restore.mapping.title') }}</h3>
          <p class="text-xs text-muted">{{ t('system.backup.full_restore.mapping.hint') }}</p>
          <div class="divide-y divide-muted border border-muted" data-testid="full-restore-mappings">
            <div v-for="root in preview.storage_roots" :key="root.id" class="grid gap-2 p-3 sm:grid-cols-2 sm:items-center">
              <div class="min-w-0">
                <p class="text-sm font-medium text-highlighted">{{ root.name }}</p>
                <p class="truncate font-mono text-xs text-muted">{{ root.path }}</p>
                <UBadge v-if="!root.native" color="warning" variant="subtle" size="sm">{{ t('system.backup.full_restore.mapping.foreign') }}</UBadge>
              </div>
              <UInput
                v-model="targets[root.id]"
                class="w-full font-mono"
                :aria-label="t('system.backup.full_restore.mapping.target', { name: root.name })"
                :placeholder="t('system.backup.full_restore.mapping.placeholder')"
              />
            </div>
          </div>
          <ul v-if="preview.paths.length" class="space-y-1 text-xs text-muted">
            <li v-for="entry in preview.paths" :key="`${entry.kind}:${entry.path}`" class="truncate font-mono">
              {{ t(`system.backup.full_restore.preview.path_kind.${entry.kind}`) }}: {{ entry.path }}
            </li>
          </ul>

          <UButton
            icon="i-lucide-flask-conical"
            color="neutral"
            variant="outline"
            :label="t('system.backup.full_restore.test.action')"
            :disabled="busy !== null"
            :loading="busy === 'test'"
            data-testid="full-restore-test"
            @click="runTest"
          />
        </section>

        <section v-if="report" class="space-y-3" data-testid="full-restore-report">
          <UAlert
            :color="report.ok ? 'success' : 'error'"
            variant="subtle"
            :icon="report.ok ? 'i-lucide-circle-check' : 'i-lucide-circle-x'"
            :description="report.ok ? t('system.backup.full_restore.test.ok') : t('system.backup.full_restore.test.failed')"
          />
          <p class="text-sm text-toned">
            {{ t('system.backup.full_restore.test.summary', { moved: report.moved_paths, credentials: report.restored_credentials, migrated: report.schema.migrated, packages: report.counts.packages }) }}
          </p>
          <p v-if="!report.problems.length" class="text-sm text-muted">{{ t('system.backup.full_restore.test.none') }}</p>
          <ul v-else class="divide-y divide-muted border border-muted">
            <li v-for="problem in report.problems" :key="`${problem.severity}:${problem.code}`" class="space-y-1 p-3 text-sm">
              <div class="flex items-center gap-2">
                <UBadge :color="problem.severity === 'error' ? 'error' : 'warning'" variant="subtle">
                  {{ t(`system.backup.full_restore.test.severity.${problem.severity}`) }}
                </UBadge>
                <span class="text-toned">{{ problemText(problem) }}</span>
              </div>
              <p v-for="example in problem.examples" :key="example" class="truncate font-mono text-xs text-muted">{{ example }}</p>
            </li>
          </ul>
          <UCheckbox
            v-if="report.ok"
            v-model="confirmed"
            :label="t('system.backup.full_restore.confirm.label')"
            data-testid="full-restore-confirm"
          />
        </section>
      </div>
    </template>
    <template #footer>
      <UButton color="neutral" variant="ghost" :label="t('common.actions.cancel')" @click="open = false" />
      <UButton
        color="warning"
        icon="i-lucide-archive-restore"
        :label="t('system.backup.full_restore.confirm.action')"
        :disabled="!canRestore"
        :loading="busy === 'restore'"
        data-testid="full-restore-start"
        @click="restore"
      />
    </template>
  </UModal>
</template>
