<script setup lang="ts">
/**
 * An automation's saved versions (RD-190-22).
 *
 * Every save stored the definition before it, and `GET /api/v1/automations/{id}/versions` had
 * answered with them since RD-090-04 — but nothing in the interface asked. The dialog lists them
 * newest first, says what each one changed against the one before it, and puts an older one
 * back in force through the ordinary update, which saves it as the newest version.
 */
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Automation, AutomationVersion, Category, NotificationTarget } from '@/api/types'
import DataState from '@/components/DataState.vue'
import { useConfirm } from '@/composables/useConfirm'
import { useFetchState } from '@/composables/useFetchState'
import { useAutomationsStore } from '@/stores/automations'
import { changedParts, describeAction, describeCondition } from '@/utils/automationText'
import { formatMoment } from '@/utils/format'
import FormFeedback from '@/components/FormFeedback.vue'

const props = defineProps<{
  automation: Automation
  /** To name what a "move to category" or "call webhook" action points at. */
  categories: Category[]
  targets: NotificationTarget[]
}>()
const emit = defineEmits<{ close: [], restored: [id: string] }>()

const { t } = useI18n()
const store = useAutomationsStore()
const confirm = useConfirm()
const versions = ref<AutomationVersion[]>([])
const { loading, load: track } = useFetchState()
const restoringId = ref<string | null>(null)
const message = ref<string | null>(null)
/** The version in force, which moves when a restore saves a new one. */
const current = ref(props.automation.version)

const names = computed(() => {
  const map = new Map<string, string>()
  for (const category of props.categories) map.set(category.id, category.name)
  for (const target of props.targets) map.set(target.id, target.name)
  return map
})

/** Each version with the parts it changed against the version saved before it. */
const entries = computed(() =>
  versions.value.map((version, index) => ({ version, changed: changedParts(version, versions.value[index + 1]) }))
)

function load(): Promise<void> {
  return track(async () => { versions.value = (await store.versions(props.automation.id)) ?? [] })
}

async function restore(version: AutomationVersion): Promise<void> {
  const confirmed = await confirm({
    title: t('automation.history.restore_title', { version: version.version }),
    description: t('automation.history.restore_description', { version: version.version, name: props.automation.name }),
    confirmLabel: t('automation.history.restore'),
    confirmIcon: 'i-lucide-history'
  })
  if (!confirmed) return
  message.value = null
  restoringId.value = version.id
  const saved = await store.restore(props.automation, version)
  restoringId.value = null
  if (!saved) return
  current.value = saved.version
  message.value = t('automation.history.restored', { version: version.version, current: saved.version })
  emit('restored', props.automation.id)
  await load()
}

onMounted(() => void load())
</script>

<template>
  <UModal
    :title="t('automation.history.title', { name: props.automation.name })"
    :description="t('automation.history.description')"
    :close="{ onClick: () => emit('close') }"
    :ui="{ content: 'sm:max-w-2xl' }"
  >
    <template #body>
      <div class="space-y-3">
        <FormFeedback :error="store.error" :message="message" />
        <ol v-if="entries.length" class="divide-y divide-muted border border-muted">
          <li v-for="{ version, changed } in entries" :key="version.id" class="space-y-2 p-3" data-testid="automation-version">
            <div class="flex flex-wrap items-center gap-2">
              <span class="text-sm font-semibold text-highlighted">{{ t('automation.version', { version: version.version }) }}</span>
              <span class="numeric text-xs text-muted">{{ formatMoment(version.created_at) }}</span>
              <UBadge v-if="version.version === current" color="primary" variant="subtle" size="sm">{{ t('automation.history.current') }}</UBadge>
              <UButton
                v-else
                class="ms-auto"
                size="xs"
                color="neutral"
                variant="outline"
                icon="i-lucide-history"
                :label="t('automation.history.restore')"
                :loading="restoringId === version.id"
                :disabled="restoringId !== null"
                @click="restore(version)"
              />
            </div>
            <dl class="grid gap-x-4 gap-y-1 text-xs sm:grid-cols-[auto_1fr]">
              <dt class="text-muted">{{ t('automation.trigger_label') }}</dt>
              <dd class="text-toned">
                {{ t(`automation.trigger.${version.trigger}`) }}
                <UBadge v-if="changed.has('trigger')" color="warning" variant="subtle" size="sm">{{ t('automation.history.changed') }}</UBadge>
              </dd>
              <dt class="text-muted">{{ t('automation.condition.heading') }}</dt>
              <dd class="break-words text-toned">
                {{ describeCondition(version.condition, t) }}
                <UBadge v-if="changed.has('condition')" color="warning" variant="subtle" size="sm">{{ t('automation.history.changed') }}</UBadge>
              </dd>
              <dt class="text-muted">{{ t('automation.action.heading') }}</dt>
              <dd class="text-toned">
                <span>{{ version.actions.map(action => describeAction(action, t, id => names.get(id))).join(' · ') }}</span>
                <UBadge v-if="changed.has('actions')" color="warning" variant="subtle" size="sm" class="ms-1">{{ t('automation.history.changed') }}</UBadge>
              </dd>
            </dl>
          </li>
        </ol>
        <DataState v-else :loading="loading" :empty="!store.error" :rows="2">
          <UEmpty :description="t('automation.history.empty')" />
        </DataState>
      </div>
    </template>
  </UModal>
</template>
