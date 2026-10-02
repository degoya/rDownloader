<script setup lang="ts">
/**
 * What a package is before it is installed (RD-140-01): name, version, publisher, the key's
 * standing, every permission it asks for, and the release notes a repository published. For an
 * update, the permissions the installed version does not have yet come first (RD-160-09).
 *
 * Opened for an uploaded file and for a repository's offer alike, and for a package from an
 * already trusted key too — installing used to be the first thing that looked at a package, so
 * those went onto disk without anybody having seen what they ask for. A key this installation
 * has not seen is confirmed right here, by sending back the fingerprint the dialog shows; the
 * separate trust prompt the upload used to open is this dialog now.
 *
 * Laid out as labelled rows (RD-180-22): the publisher folds its key id and fingerprint away
 * behind its trust line, open from the start when the key still has to be confirmed — that
 * fingerprint is what installing confirms — and the package digest shows its two ends until
 * somebody unfolds the whole of it.
 */
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import {
  groupFingerprint,
  install as installPackage,
  preview as loadPreview,
  type PluginPreview,
  type PreviewSource
} from '@/api/pluginRepositories'
import { currentLocale } from '@/i18n'
import { loadPluginMessages, resetPluginMessages } from '@/i18n/plugins'
import { serverMessageFrom, translateServerMessage } from '@/i18n/server'
import { capabilityLabel as labelOf, permissionLabels } from './pluginPermissions'

const props = defineProps<{
  /** The package to preview; `null` keeps the dialog closed. */
  source: PreviewSource | null
}>()
const emit = defineEmits<{
  close: []
  /** The service's own sentence about the install, already translated. */
  installed: [message: string]
}>()

const { t } = useI18n()
const preview = ref<PluginPreview | null>(null)
const loading = ref(false)
const installing = ref(false)
const error = ref<string | null>(null)

const open = computed({
  get: () => props.source !== null,
  set: (value: boolean) => {
    if (!value) emit('close')
  }
})

/** A key nobody confirmed yet is confirmed by installing; the button says so. */
const needsTrust = computed(() => preview.value?.key_status === 'untrusted')
const keyIcon = computed(() => preview.value?.key_status === 'trusted' ? 'i-lucide-shield-check' : 'i-lucide-shield-alert')
/** The trust line's colour as text, the icon's beside it. */
const keyText = computed(() => {
  switch (preview.value?.key_status) {
    case 'trusted': return 'text-success'
    case 'untrusted': return 'text-warning'
    default: return 'text-error'
  }
})
const keyColor = computed(() => {
  switch (preview.value?.key_status) {
    case 'trusted': return 'success'
    case 'untrusted': return 'warning'
    default: return 'error'
  }
})

watch(() => props.source, source => { void load(source) }, { immediate: true })

async function load(source: PreviewSource | null): Promise<void> {
  preview.value = null
  error.value = null
  loading.value = source !== null
  if (!source) return
  const answer = await loadPreview(source)
  // A dialog closed or pointed elsewhere while this was in flight keeps what it shows now.
  if (props.source !== source) return
  loading.value = false
  if (answer.ok) preview.value = answer.data
  else error.value = translateServerMessage(answer.message)
}

async function install(): Promise<void> {
  const source = props.source
  const shown = preview.value
  if (!source || !shown?.installable) return
  installing.value = true
  error.value = null
  const fingerprint = needsTrust.value ? shown.publisher?.fingerprint : undefined
  const answer = await installPackage(source, fingerprint)
  installing.value = false
  if (!answer.ok) {
    error.value = translateServerMessage(answer.message)
    return
  }
  // A new plugin brings its own translations along.
  resetPluginMessages()
  await loadPluginMessages(currentLocale())
  emit('installed', translateServerMessage(serverMessageFrom(answer.data)))
}

function capabilityLabel(capability: string): string {
  return labelOf(t, capability)
}

/** Where the package comes from: a repository by name, or an upload. */
const sourceLabel = computed(() => preview.value?.source
  ? t('plugins.preview.source_repository', { repository: preview.value.source.repository_name })
  : t('plugins.preview.source_upload'))

/**
 * A 64-digit hash as two lines of four 8-digit groups, the way it is compared by eye; the
 * line break is kept by `whitespace-pre-line` and a narrow screen may still wrap at a space.
 */
function hashLines(hash: string): string {
  const groups = groupFingerprint(hash).split(' ')
  const lines: string[] = []
  for (let index = 0; index < groups.length; index += 4) lines.push(groups.slice(index, index + 4).join(' '))
  return lines.join('\n')
}

/** The digest's two ends, enough to compare at a glance; the whole of it is one click away. */
const shortDigest = computed(() => {
  const digest = preview.value?.package_digest ?? ''
  return digest.length > 16 ? `${digest.slice(0, 8)} … ${digest.slice(-8)}` : digest
})

/** What an update asks for beyond the installed version; `null` for a plugin not installed yet. */
const added = computed(() => {
  const change = preview.value?.added_permissions
  if (!change) return null
  return { version: change.installed_version, labels: permissionLabels(t, change.permissions) }
})
</script>

<template>
  <UModal v-model:open="open" :title="t('plugins.preview.title')" :ui="{ footer: 'flex-col items-stretch gap-3 sm:flex-row sm:items-center sm:justify-between' }">
    <template #body>
      <div v-if="source" class="space-y-4">
        <p v-if="loading" class="text-sm text-muted">{{ t('common.data.loading') }}</p>
        <UAlert v-if="error" color="error" variant="subtle" :description="error" />
        <template v-if="preview">
          <div>
            <div class="flex flex-wrap items-center gap-2">
              <p class="font-semibold text-highlighted">{{ preview.name }}</p>
              <UBadge class="font-mono" color="primary" variant="subtle">v{{ preview.version }}</UBadge>
              <UBadge color="neutral" variant="subtle">{{ t(`plugins.type.${preview.plugin_type}`) }}</UBadge>
              <p v-if="!preview.installed_versions.length" class="ml-auto text-xs text-muted" data-preview-source>{{ sourceLabel }}</p>
            </div>
            <p class="mt-1 text-sm leading-5 text-toned">{{ preview.description }}</p>
            <p v-if="preview.installed_versions.length" class="mt-1 flex flex-wrap gap-x-1.5 text-xs text-muted" data-preview-source>
              <span>{{ sourceLabel }}</span>
              <span aria-hidden="true">·</span>
              <span>{{ t('plugins.preview.installed_versions', { versions: preview.installed_versions.join(', ') }) }}</span>
            </p>
          </div>

          <UAlert v-if="preview.withdrawn" color="error" variant="subtle" :description="t('plugins.preview.withdrawn')" />
          <UAlert v-if="preview.incompatible" color="error" variant="subtle" :description="t('plugins.preview.incompatible', { reason: preview.incompatible })" />

          <UCollapsible
            v-if="preview.publisher"
            :default-open="needsTrust"
            class="rounded-md border border-default bg-elevated/50"
            data-preview-publisher
          >
            <template #default="{ open: unfolded }">
              <button type="button" class="flex w-full items-center gap-3 p-3 text-left" :aria-expanded="unfolded">
                <UIcon :name="keyIcon" :class="['size-5 shrink-0', keyText]" />
                <span class="min-w-0 flex-1">
                  <span class="flex flex-wrap items-baseline gap-x-2">
                    <span class="text-[10px] uppercase tracking-wide text-muted">{{ t('plugins.preview.publisher') }}</span>
                    <span class="text-sm font-medium text-highlighted">{{ t('plugins.preview.author', { author: preview.publisher.author }) }}</span>
                  </span>
                  <span :class="['block text-xs', keyText]">{{ t(`plugins.preview.key.${preview.key_status}`) }}</span>
                </span>
                <UIcon :name="unfolded ? 'i-lucide-chevron-down' : 'i-lucide-chevron-right'" class="size-4 shrink-0 text-muted" />
              </button>
            </template>
            <template #content>
              <dl class="grid gap-x-3 gap-y-1 border-t border-default p-3 text-xs sm:grid-cols-[7rem_minmax(0,1fr)] sm:gap-y-2">
                <dt class="text-muted">{{ t('plugins.trust.key_id') }}</dt>
                <dd class="font-mono text-highlighted">{{ preview.publisher.key_id }}</dd>
                <dt class="text-muted">{{ t('plugins.trust.fingerprint') }}</dt>
                <dd class="whitespace-pre-line font-mono text-highlighted">{{ hashLines(preview.publisher.fingerprint) }}</dd>
              </dl>
            </template>
          </UCollapsible>
          <div v-else class="flex items-center gap-3 rounded-md border border-default bg-elevated/50 p-3" data-preview-publisher>
            <UIcon :name="keyIcon" :class="['size-5 shrink-0', keyText]" />
            <span class="min-w-0 flex-1">
              <span class="block text-[10px] uppercase tracking-wide text-muted">{{ t('plugins.preview.publisher') }}</span>
              <UBadge class="mt-1" :color="keyColor" variant="subtle">{{ t(`plugins.preview.key.${preview.key_status}`) }}</UBadge>
            </span>
          </div>
          <UAlert v-if="needsTrust" color="warning" variant="subtle" :description="t('plugins.trust.warning')" />

          <dl class="grid items-start gap-x-3 gap-y-1 sm:grid-cols-[8.5rem_minmax(0,1fr)] sm:gap-y-2" data-preview-permissions>
            <dt class="pt-1 text-[10px] uppercase tracking-wide text-muted">{{ t('plugins.preview.permissions') }}</dt>
            <dd v-if="preview.permissions.granted.length" class="flex flex-wrap gap-1">
              <UBadge v-for="capability in preview.permissions.granted" :key="capability" color="warning" variant="outline">{{ capabilityLabel(capability) }}</UBadge>
            </dd>
            <dd v-else class="text-sm text-toned">{{ t('plugins.preview.no_permissions') }}</dd>
            <template v-if="preview.permissions.http_domains.length">
              <dt class="pt-1.5 text-xs text-muted sm:pt-0.5">{{ t('plugins.preview.domains') }}</dt>
              <dd class="flex flex-wrap gap-1">
                <UBadge v-for="domain in preview.permissions.http_domains" :key="domain" color="neutral" variant="outline">{{ domain }}</UBadge>
              </dd>
            </template>
            <template v-if="preview.permissions.stream_hosts.length">
              <dt class="pt-1.5 text-xs text-muted sm:pt-0.5">{{ t('plugins.preview.stream_hosts') }}</dt>
              <dd class="flex flex-wrap gap-1">
                <UBadge v-for="host in preview.permissions.stream_hosts" :key="host" color="neutral" variant="outline">{{ host }}</UBadge>
              </dd>
            </template>
          </dl>

          <dl v-if="added" class="grid items-start gap-x-3 gap-y-1 sm:grid-cols-[8.5rem_minmax(0,1fr)]" data-preview-added-permissions>
            <dt class="pt-1 text-[10px] uppercase tracking-wide text-muted">{{ t('plugins.preview.added_permissions', { version: added.version }) }}</dt>
            <dd v-if="added.labels.length" class="flex flex-wrap gap-1">
              <UBadge v-for="label in added.labels" :key="label" color="error" variant="subtle" icon="i-lucide-shield-alert">{{ label }}</UBadge>
            </dd>
            <dd v-else class="text-sm text-toned">{{ t('plugins.preview.no_added_permissions', { version: added.version }) }}</dd>
          </dl>

          <!-- Plain text from the index, shown as text: never markup, whoever wrote it. -->
          <dl v-if="preview.release_notes" class="grid items-start gap-x-3 gap-y-1 sm:grid-cols-[8.5rem_minmax(0,1fr)]">
            <dt class="pt-1 text-[10px] uppercase tracking-wide text-muted">{{ t('plugins.preview.release_notes') }}</dt>
            <dd class="whitespace-pre-line break-words text-sm leading-6 text-toned">{{ preview.release_notes }}</dd>
          </dl>

          <UCollapsible class="border-t border-default pt-3" data-preview-digest>
            <template #default="{ open: unfolded }">
              <button type="button" class="flex w-full items-center gap-2 text-left" :aria-expanded="unfolded">
                <UIcon :name="unfolded ? 'i-lucide-chevron-down' : 'i-lucide-chevron-right'" class="size-4 shrink-0 text-muted" />
                <span class="flex-1 text-[10px] uppercase tracking-wide text-muted">{{ t('plugins.preview.digest') }}</span>
                <span v-if="!unfolded" class="font-mono text-xs text-muted">{{ shortDigest }}</span>
              </button>
            </template>
            <template #content>
              <p class="mt-2 whitespace-pre-line pl-6 font-mono text-xs text-toned">{{ hashLines(preview.package_digest) }}</p>
            </template>
          </UCollapsible>
        </template>
      </div>
    </template>
    <template #footer>
      <template v-if="source">
        <p class="text-xs text-muted">
          <template v-if="preview">{{ t('plugins.preview.restart') }}</template>
        </p>
        <div class="flex shrink-0 items-center justify-end gap-2">
          <UButton color="neutral" variant="outline" :label="t('plugins.trust.cancel')" @click="open = false" />
          <UButton
            color="primary"
            :icon="needsTrust ? 'i-lucide-shield-check' : 'i-lucide-package-plus'"
            :label="needsTrust ? t('plugins.trust.confirm') : t('plugins.preview.install')"
            :disabled="!preview?.installable"
            :loading="installing"
            @click="install"
          />
        </div>
      </template>
    </template>
  </UModal>
</template>
