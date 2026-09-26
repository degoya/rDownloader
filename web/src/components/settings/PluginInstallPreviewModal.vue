<script setup lang="ts">
/**
 * What a package is before it is installed (RD-140-01): name, version, publisher, the key's
 * standing, every permission it asks for, and the release notes a repository published.
 *
 * Opened for an uploaded file and for a repository's offer alike, and for a package from an
 * already trusted key too — installing used to be the first thing that looked at a package, so
 * those went onto disk without anybody having seen what they ask for. A key this installation
 * has not seen is confirmed right here, by sending back the fingerprint the dialog shows; the
 * separate trust prompt the upload used to open is this dialog now.
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

/**
 * A grant as the manifest declares it. Two carry a detail worth showing in full:
 * `secrets:<reference>` names the one credential the plugin may expand, and `net_stream:<ports>`
 * the ports it may dial. The rest are fixed capability names.
 */
function capabilityLabel(capability: string): string {
  const [name, detail] = capability.split(/:(.*)/s)
  if (name === 'secrets') return t('plugins.capability.secret', { reference: detail })
  if (name === 'net_stream') return t('plugins.capability.net_stream', { ports: detail })
  return t(`plugins.capability.${name}`)
}
</script>

<template>
  <UModal v-model:open="open" :title="t('plugins.preview.title')">
    <template #body>
      <div v-if="source" class="space-y-4">
        <p v-if="loading" class="text-sm text-muted">{{ t('common.data.loading') }}</p>
        <UAlert v-if="error" color="error" variant="subtle" :description="error" />
        <template v-if="preview">
          <div>
            <div class="flex flex-wrap items-center gap-2">
              <p class="font-medium text-highlighted">{{ preview.name }}</p>
              <UBadge color="primary" variant="subtle">v{{ preview.version }}</UBadge>
              <UBadge color="neutral" variant="subtle">{{ t(`plugins.type.${preview.plugin_type}`) }}</UBadge>
            </div>
            <p class="mt-1 text-sm leading-5 text-toned">{{ preview.description }}</p>
            <p class="mt-1 text-xs text-muted">
              {{ preview.source ? t('plugins.preview.source_repository', { repository: preview.source.repository_name }) : t('plugins.preview.source_upload') }}
            </p>
            <p v-if="preview.installed_versions.length" class="mt-1 text-xs text-muted">
              {{ t('plugins.preview.installed_versions', { versions: preview.installed_versions.join(', ') }) }}
            </p>
          </div>

          <UAlert v-if="preview.withdrawn" color="error" variant="subtle" :description="t('plugins.preview.withdrawn')" />
          <UAlert v-if="preview.incompatible" color="error" variant="subtle" :description="t('plugins.preview.incompatible', { reason: preview.incompatible })" />

          <div class="border border-muted bg-elevated p-3" data-preview-publisher>
            <p class="text-[10px] uppercase tracking-wide text-muted">{{ t('plugins.preview.publisher') }}</p>
            <template v-if="preview.publisher">
              <p class="mt-1 text-sm text-highlighted">{{ t('plugins.preview.author', { author: preview.publisher.author }) }}</p>
              <p class="mt-2 text-xs text-muted">{{ t('plugins.trust.key_id') }}</p>
              <p class="font-mono text-sm text-highlighted">{{ preview.publisher.key_id }}</p>
              <p class="mt-2 text-xs text-muted">{{ t('plugins.trust.fingerprint') }}</p>
              <p class="break-all font-mono text-sm text-highlighted">{{ groupFingerprint(preview.publisher.fingerprint) }}</p>
            </template>
            <UBadge class="mt-3" :color="keyColor" variant="subtle">{{ t(`plugins.preview.key.${preview.key_status}`) }}</UBadge>
          </div>
          <UAlert v-if="needsTrust" color="warning" variant="subtle" :description="t('plugins.trust.warning')" />

          <div data-preview-permissions>
            <p class="text-[10px] uppercase tracking-wide text-muted">{{ t('plugins.preview.permissions') }}</p>
            <div v-if="preview.permissions.granted.length" class="mt-1 flex flex-wrap gap-1">
              <UBadge v-for="capability in preview.permissions.granted" :key="capability" color="warning" variant="outline">{{ capabilityLabel(capability) }}</UBadge>
            </div>
            <p v-else class="mt-1 text-sm text-toned">{{ t('plugins.preview.no_permissions') }}</p>
            <template v-if="preview.permissions.http_domains.length">
              <p class="mt-2 text-xs text-muted">{{ t('plugins.preview.domains') }}</p>
              <div class="mt-1 flex flex-wrap gap-1">
                <UBadge v-for="domain in preview.permissions.http_domains" :key="domain" color="neutral" variant="outline">{{ domain }}</UBadge>
              </div>
            </template>
            <template v-if="preview.permissions.stream_hosts.length">
              <p class="mt-2 text-xs text-muted">{{ t('plugins.preview.stream_hosts') }}</p>
              <div class="mt-1 flex flex-wrap gap-1">
                <UBadge v-for="host in preview.permissions.stream_hosts" :key="host" color="neutral" variant="outline">{{ host }}</UBadge>
              </div>
            </template>
          </div>

          <!-- Plain text from the index, shown as text: never markup, whoever wrote it. -->
          <div v-if="preview.release_notes">
            <p class="text-[10px] uppercase tracking-wide text-muted">{{ t('plugins.preview.release_notes') }}</p>
            <p class="mt-1 whitespace-pre-line break-words text-sm leading-6 text-toned">{{ preview.release_notes }}</p>
          </div>

          <div>
            <p class="text-[10px] uppercase tracking-wide text-muted">{{ t('plugins.preview.digest') }}</p>
            <p class="break-all font-mono text-[11px] text-muted">{{ groupFingerprint(preview.package_digest) }}</p>
          </div>
          <p class="text-xs text-muted">{{ t('plugins.preview.restart') }}</p>
        </template>
      </div>
    </template>
    <template #footer>
      <div v-if="source" class="flex w-full justify-end gap-2">
        <UButton color="neutral" variant="ghost" :label="t('plugins.trust.cancel')" @click="open = false" />
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
  </UModal>
</template>
