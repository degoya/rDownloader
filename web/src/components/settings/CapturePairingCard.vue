<script setup lang="ts">
import { computed, ref } from 'vue'
import { useToast } from '@nuxt/ui/composables'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { CaptureToken } from '@/api/types'
import DataState from '@/components/DataState.vue'
import { useConfirm } from '@/composables/useConfirm'
import { BASE_PATH } from '@/basePath'
import { formatDay } from '@/utils/format'
import SectionHeader from '@/components/SectionHeader.vue'

// The parent owns the list so it can keep its own derived state (status cards, wizard badges).
const agents = defineModel<CaptureToken[]>({ required: true })
const props = defineProps<{
  /** True while the owner is still fetching the agent list; the empty state waits (RD-104-07). */
  loading?: boolean | undefined
  /** The fetch's failure, so an unreachable service is not drawn as "no agents paired". */
  loadError?: string | null | undefined
}>()
const { t } = useI18n()
const confirm = useConfirm()
const toast = useToast()

const pairLabel = ref('Windows 11')
const bearer = ref<string | null>(null)
const bearerTokenId = ref<string | null>(null)
const pairError = ref<string | null>(null)
const pairing = ref(false)
const revokingId = ref<string | null>(null)

const serverOrigin = `${window.location.origin}${BASE_PATH}`
const captureCommand = computed(() => bearer.value
  ? `rdownloader-capture configure --service "${serverOrigin}" --token "${bearer.value}"`
  : '')

async function pair(): Promise<void> {
  pairing.value = true
  pairError.value = null
  const response = await api.POST('/api/v1/capture/pair', { body: { label: pairLabel.value } })
  pairing.value = false
  if (response.data) {
    bearer.value = response.data.bearer
    bearerTokenId.value = response.data.token.id
    agents.value = [response.data.token, ...agents.value]
  } else {
    pairError.value = responseError(response)
  }
}

async function copyCommand(): Promise<void> {
  if (!captureCommand.value) return
  await navigator.clipboard.writeText(captureCommand.value)
  toast.add({
    title: t('system.pairing.copied_title'),
    description: t('system.pairing.copied_description'),
    color: 'success',
    icon: 'i-lucide-copy-check'
  })
}

async function copyToken(): Promise<void> {
  if (!bearer.value) return
  await navigator.clipboard.writeText(bearer.value)
  toast.add({
    title: t('system.pairing.copied_title'),
    description: t('system.pairing.token_copied_description'),
    color: 'success'
  })
}

async function revokeAgent(agent: CaptureToken): Promise<void> {
  const confirmed = await confirm({
    title: t('system.revoke.title'),
    description: t('system.revoke.description', { label: agent.label }),
    confirmLabel: t('system.revoke.confirm'),
    confirmIcon: 'i-lucide-unplug',
    destructive: true
  })
  if (!confirmed) return
  revokingId.value = agent.id
  const response = await api.DELETE('/api/v1/capture/agents/{id}', {
    params: { path: { id: agent.id } }
  })
  revokingId.value = null
  if (!response.data) {
    pairError.value = responseError(response)
    return
  }
  agents.value = agents.value.filter(item => item.id !== agent.id)
  if (bearerTokenId.value === agent.id) {
    bearer.value = null
    bearerTokenId.value = null
  }
  toast.add({ title: t('system.revoke.done'), color: 'success', icon: 'i-lucide-unplug' })
}

</script>

<template>
  <div class="grid gap-6 lg:grid-cols-[minmax(280px,0.7fr)_minmax(360px,1.3fr)]">
    <div>
      <SectionHeader :eyebrow="t('system.pairing.eyebrow')" :title="t('system.pairing.title')" />
      <i18n-t keypath="system.pairing.description" tag="p" class="mt-2 text-sm leading-6 text-muted">
        <template #scope><span class="font-mono">capture:*</span></template>
      </i18n-t>
      <form class="mt-4 flex gap-2" @submit.prevent="pair">
        <UInput v-model="pairLabel" required maxlength="100" icon="i-lucide-monitor" class="flex-1" />
        <UButton type="submit" icon="i-lucide-link" :label="t('system.pairing.submit')" :loading="pairing" />
      </form>
      <UAlert v-if="pairError" class="mt-3" color="error" variant="subtle" :description="pairError" />
      <div v-if="bearer" class="mt-3 border border-warning/40 bg-warning/10 p-3">
        <p class="mb-2 text-xs font-medium text-warning">{{ t('system.pairing.copy_hint') }}</p>
        <div class="flex items-start gap-2">
          <code class="min-w-0 flex-1 break-all font-mono text-xs leading-5 text-highlighted">{{ captureCommand }}</code>
          <UButton icon="i-lucide-copy" :label="t('system.pairing.copy_command')" color="neutral" variant="soft" @click="copyCommand" />
        </div>
        <p class="mt-2 font-mono text-[10px] leading-5 text-muted">{{ t('system.pairing.afterwards') }}<br>rdownloader-capture autostart install<br>rdownloader-capture association install</p>
        <div class="mt-3 border-t border-warning/30 pt-3">
          <p class="mb-2 text-xs font-medium text-warning">{{ t('system.pairing.extension_hint') }}</p>
          <div class="flex items-start gap-2">
            <code class="min-w-0 flex-1 break-all font-mono text-xs leading-5 text-highlighted">{{ bearer }}</code>
            <UButton icon="i-lucide-copy" :label="t('system.pairing.copy_token')" color="neutral" variant="soft" @click="copyToken" />
          </div>
          <p class="mt-2 text-[11px] leading-5 text-muted">{{ t('system.pairing.extension_steps', { origin: serverOrigin }) }}</p>
        </div>
      </div>
    </div>
    <div>
      <p class="eyebrow mb-3">{{ t('system.agents.eyebrow') }}</p>
      <div v-if="agents.length" class="divide-y divide-muted border border-muted">
        <div v-for="agent in agents" :key="agent.id" class="flex items-center gap-3 p-3">
          <span class="size-2 bg-success" />
          <div class="min-w-0 flex-1"><p class="truncate text-sm font-medium text-highlighted">{{ agent.label }}</p><p class="font-mono text-[11px] text-muted">{{ agent.scopes.join(', ') }}</p></div>
          <span class="numeric text-[11px] text-muted">{{ formatDay(agent.created_at) }}</span>
          <UButton
            icon="i-lucide-trash-2"
            :aria-label="t('system.agents.revoke')"
            color="error"
            variant="ghost"
            size="xs"
            :loading="revokingId === agent.id"
            @click="revokeAgent(agent)"
          />
        </div>
      </div>
      <DataState v-else :loading="props.loading" :error="props.loadError" :empty="true" :rows="2">
        <p class="border border-dashed border-muted p-6 text-center text-sm text-muted">{{ t('system.agents.empty') }}</p>
      </DataState>
    </div>
  </div>
</template>
