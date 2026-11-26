<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { COLLISION_DECISIONS, decideCollision, listCollisionPrompts, type CollisionDecision, type CollisionPrompt } from '@/api/storage'
import { translateServerMessage } from '@/i18n/server'
import { useTransfersStore } from '@/stores/transfers'
import { formatBytes } from '@/utils/format'

/**
 * The downloads waiting for a collision decision (RD-150-01). Each one waits alone: the rest of
 * the queue keeps running, and the question survives a restart because it is a row. A download
 * that asks is blocked and one that was answered no longer is, so the questions are read again
 * whenever the blocked files change; polling asked every five seconds (RD-1480-18).
 */
const { t } = useI18n()
const prompts = ref<CollisionPrompt[]>([])
const error = ref<string | null>(null)
const deciding = ref<string | null>(null)
const transfers = useTransfersStore()
// With the moment of the change: a file asked again between two reads of the queue stays blocked.
const blocked = computed(() => transfers.downloads
  .filter(file => file.state === 'blocked')
  .map(file => `${file.id}@${file.updated_at}`)
  .join(' '))

async function load(): Promise<void> {
  const answer = await listCollisionPrompts()
  if (answer.ok) prompts.value = answer.data.filter(prompt => prompt.decision === null)
}

async function decide(prompt: CollisionPrompt, decision: CollisionDecision): Promise<void> {
  deciding.value = `${prompt.download_id}:${decision}`
  error.value = null
  const answer = await decideCollision(prompt.download_id, decision)
  deciding.value = null
  if (!answer.ok) {
    error.value = translateServerMessage(answer.message) || t('downloads.collision.prompts.failed')
    return
  }
  await load()
}

const DECISION_ICONS: Record<CollisionDecision, string> = {
  rename: 'i-lucide-copy-plus',
  skip: 'i-lucide-skip-forward',
  overwrite: 'i-lucide-file-pen-line'
}

onMounted(() => void load())
watch(blocked, () => void load())

defineExpose({ reload: load })
</script>

<template>
  <div v-if="prompts.length" class="flex flex-col gap-2" data-testid="collision-prompts">
    <UAlert v-if="error" color="error" role="alert" icon="i-lucide-circle-alert" :description="error" />
    <UAlert
      v-for="prompt in prompts"
      :key="prompt.download_id"
      color="warning"
      icon="i-lucide-files"
      :title="t('downloads.collision.prompts.title', { name: prompt.target_name })"
    >
      <template #description>
        <p>
          {{ t(prompt.phase === 'after_transfer' ? 'downloads.collision.prompts.after_transfer' : 'downloads.collision.prompts.before_transfer', {
            package: prompt.package_name ?? '',
            size: prompt.existing_bytes === null ? '?' : formatBytes(String(prompt.existing_bytes))
          }) }}
        </p>
        <p class="mt-1 text-xs">{{ t('downloads.collision.prompts.only_this') }}</p>
      </template>
      <template #actions>
        <UButton
          v-for="decision in COLLISION_DECISIONS"
          :key="decision"
          size="xs"
          :color="decision === 'overwrite' ? 'error' : 'warning'"
          :variant="decision === 'rename' ? 'solid' : 'outline'"
          :icon="DECISION_ICONS[decision]"
          :label="t(`downloads.collision.decisions.${decision}`)"
          :loading="deciding === `${prompt.download_id}:${decision}`"
          :disabled="deciding !== null"
          @click="decide(prompt, decision)"
        />
      </template>
    </UAlert>
  </div>
</template>
