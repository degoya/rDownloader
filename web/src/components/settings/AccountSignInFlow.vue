<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { AuthFlow } from '@/api/types'
import { isOpenFlow } from '@/composables/useAuthFlows'
import { translateServerMessage } from '@/i18n/server'
import { formatMoment } from '@/utils/format'
import { safeHttpUrl } from '@/utils/safeUrl'

/**
 * Where an account's sign-in stands (RD-090-13, RD-150-09).
 *
 * While the service waits, the address, the code and a live status stay here until the flow
 * ends — signed in, refused, expired or cancelled. The address is a link the person follows
 * themselves; nothing is opened for them.
 */
const props = defineProps<{ flow: AuthFlow, provider: string }>()
const emit = defineEmits<{ cancel: [] }>()

const { t } = useI18n()
const open = computed(() => isOpenFlow(props.flow))
const expires = computed(() => formatMoment(props.flow.expires_at))

/**
 * Why a flow ended, in the reader's language (RD-106-02).
 *
 * Two kinds of text arrive in `message`. What the service decided by itself — no installed
 * plugin claims this provider, the provider stayed unreachable — is a stable code, because
 * there is no foreign answer to quote and a code can be translated. What a plugin reported
 * is the provider's own English wording, which nothing here can translate.
 * `translateServerMessage` takes the first and falls through to the second.
 */
const failure = computed(() => {
  const message = props.flow.message
  if (!message) return t('network.account.connect_failed')
  return translateServerMessage({ code: message, message, params: { provider: props.provider } })
})
</script>

<template>
  <div class="mt-2 border border-muted bg-elevated p-3" data-testid="sign-in-flow">
    <template v-if="open">
      <p class="text-xs leading-5 text-muted">{{ t('network.account.connect_instructions') }}</p>
      <ULink
        v-if="flow.verification_url"
        :to="safeHttpUrl(flow.verification_url)"
        target="_blank"
        rel="noopener noreferrer"
        class="mt-2 block break-all font-mono text-sm text-highlighted underline"
      >{{ flow.verification_url }}</ULink>
      <p v-if="flow.user_code" class="mt-1 font-mono text-lg font-semibold tracking-widest text-primary">
        {{ flow.user_code }}
      </p>
      <p class="mt-2 flex items-start gap-2 text-xs leading-5 text-muted" role="status" data-testid="sign-in-flow-status">
        <UIcon name="i-lucide-loader-circle" class="mt-0.5 size-4 shrink-0 animate-spin" />
        <span>{{ t('network.account.connect_waiting') }}</span>
      </p>
      <p class="mt-1 text-xs leading-5 text-muted">{{ t('network.account.connect_polling', { provider }) }}</p>
      <p v-if="expires" class="mt-1 text-xs leading-5 text-muted">{{ t('network.account.connect_expires', { time: expires }) }}</p>
      <UButton class="mt-2" size="xs" color="neutral" variant="ghost" icon="i-lucide-x" :label="t('network.account.connect_cancel')" @click="emit('cancel')" />
    </template>
    <p v-else-if="flow.state === 'authorized'" class="text-xs leading-5 text-success">
      {{ t('network.account.connect_done') }}
    </p>
    <p v-else class="text-xs leading-5 text-error">
      {{ failure }}
    </p>
  </div>
</template>
