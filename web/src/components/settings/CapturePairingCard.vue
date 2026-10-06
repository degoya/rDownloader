<script setup lang="ts">
import { computed, ref } from 'vue'
import { useToast } from '@nuxt/ui/composables'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { CaptureToken } from '@/api/types'
import CopyField from '@/components/CopyField.vue'
import DataState from '@/components/DataState.vue'
import { useConfirm } from '@/composables/useConfirm'
import { serviceUrl } from '@/basePath'
import { formatDay } from '@/utils/format'
import { expiresInDays, tokenExpired, tokenExpiryItems, tokenExpiryLabel } from '@/utils/tokenExpiry'
import FormActions from '@/components/FormActions.vue'
import FormListLayout from '@/components/FormListLayout.vue'
import SectionHeader from '@/components/SectionHeader.vue'

// The parent owns the list so it can keep its own derived state (status cards, wizard badges).
const agents = defineModel<CaptureToken[]>({ required: true })
const props = defineProps<{
  /** True while the owner is still fetching the agent list; the empty state waits (RD-104-07). */
  loading?: boolean | undefined
  /** The fetch's failure, so an unreachable service is not drawn as "no agents paired". */
  loadError?: string | null | undefined
  /**
   * Pairs a browser extension rather than the desktop agent (RD-150-17): the heading and the
   * default name say so, and the agent's configure command is left out — the extension takes
   * the raw token.
   */
  extension?: boolean | undefined
}>()
const { t } = useI18n()
const confirm = useConfirm()
const toast = useToast()

const pairLabel = ref(props.extension ? t('system.extension.default_label') : 'Windows 11')
// Opt-in, and only for the desktop agent: pausing the queue from its tray (RD-1100-06). An agent
// paired without it can do exactly what it could before.
const queueControl = ref(false)
/// Days until the new token expires; `0`, never, is the default (RD-1110-07).
const expiryDays = ref(0)
const expiryItems = computed(() => tokenExpiryItems(t))
const bearer = ref<string | null>(null)
const bearerTokenId = ref<string | null>(null)
const pairError = ref<string | null>(null)
const pairing = ref(false)
const revokingId = ref<string | null>(null)

const serverOrigin = serviceUrl()
const captureCommand = computed(() => bearer.value
  ? `rdownloader-capture configure --service "${serverOrigin}" --token "${bearer.value}"`
  : '')

async function pair(): Promise<void> {
  pairing.value = true
  pairError.value = null
  const response = await api.POST('/api/v1/capture/pair', {
    body: {
      label: pairLabel.value,
      queue_control: !props.extension && queueControl.value,
      expires_in_days: expiresInDays(expiryDays.value)
    }
  })
  pairing.value = false
  if (response.data) {
    bearer.value = response.data.bearer
    bearerTokenId.value = response.data.token.id
    agents.value = [response.data.token, ...agents.value]
  } else {
    pairError.value = responseError(response)
  }
}

function commandCopied(): void {
  toast.add({
    title: t('system.pairing.copied_title'),
    description: t('system.pairing.copied_description'),
    color: 'success',
    icon: 'i-lucide-copy-check'
  })
}

function tokenCopied(): void {
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
  <FormListLayout :list-title="t('system.agents.eyebrow')" :count="agents.length">
    <template #form>
      <SectionHeader :eyebrow="t('system.pairing.eyebrow')" :title="props.extension ? t('system.extension.pair_title') : t('system.pairing.title')">
        <template #description>
          <i18n-t keypath="system.pairing.description" tag="span">
            <template #scope><span class="font-mono">capture:*</span></template>
          </i18n-t>
        </template>
      </SectionHeader>
      <UAlert v-if="pairError" class="mt-4" color="error" :description="pairError" />
      <form class="mt-4 space-y-3" @submit.prevent="pair">
        <UFormField :label="t('system.pairing.label')" required>
          <UInput v-model="pairLabel" required maxlength="100" icon="i-lucide-monitor" class="w-full" />
        </UFormField>
        <UCheckbox
          v-if="!props.extension"
          v-model="queueControl"
          :label="t('system.pairing.queue_control')"
          :description="t('system.pairing.queue_control_help')"
          data-testid="pairing-queue-control"
        />
        <UFormField :label="t('system.token_expiry.label')" :description="t('system.token_expiry.hint')">
          <USelect v-model="expiryDays" :items="expiryItems" icon="i-lucide-calendar-clock" class="w-full" data-testid="pairing-expiry" />
        </UFormField>
        <FormActions :create-label="t('system.pairing.submit')" create-icon="i-lucide-link" :loading="pairing" />
      </form>
      <UAlert v-if="bearer" class="mt-3" color="warning" :title="t('system.pairing.copy_hint')">
        <template #description>
          <template v-if="!props.extension">
            <CopyField class="mt-1" :value="captureCommand" :label="t('system.pairing.copy_command')" @copied="commandCopied" />
            <p class="mt-2 font-mono text-2xs leading-5 text-muted">{{ t('system.pairing.afterwards') }}<br>rdownloader-capture autostart install<br>rdownloader-capture association install</p>
            <USeparator class="my-3" :ui="{ border: 'border-warning/30' }" />
          </template>
          <p class="mb-2 text-xs font-medium text-warning">{{ t('system.pairing.extension_hint') }}</p>
          <CopyField :value="bearer!" :label="t('system.pairing.copy_token')" @copied="tokenCopied" />
          <p class="mt-2 text-2xs leading-5 text-muted">{{ t('system.pairing.extension_steps', { origin: serverOrigin }) }}</p>
        </template>
      </UAlert>
    </template>
    <template #list>
      <div v-if="agents.length" class="divide-y divide-muted border border-muted">
        <div v-for="agent in agents" :key="agent.id" class="flex items-center gap-3 p-3">
          <UChip standalone :color="tokenExpired(agent.expires_at) ? 'error' : 'success'" />
          <div class="min-w-0 flex-1">
            <p class="truncate text-sm font-medium text-highlighted">{{ agent.label }}</p>
            <p class="font-mono text-2xs text-muted">{{ agent.scopes.join(', ') }}</p>
            <p v-if="agent.expires_at" class="text-2xs" :class="tokenExpired(agent.expires_at) ? 'text-error' : 'text-muted'">{{ tokenExpiryLabel(agent.expires_at, t) }}</p>
          </div>
          <span class="numeric text-2xs text-muted">{{ formatDay(agent.created_at) }}</span>
          <UButton
            icon="i-lucide-trash-2"
            :aria-label="t('system.agents.revoke')"
            :title="t('system.agents.revoke')"
            color="error"
            variant="ghost"
            size="xs"
            :loading="revokingId === agent.id"
            @click="revokeAgent(agent)"
          />
        </div>
      </div>
      <DataState v-else :loading="props.loading" :error="props.loadError" :empty="true" :rows="2">
        <UEmpty :description="t('system.agents.empty')" />
      </DataState>
    </template>
  </FormListLayout>
</template>
