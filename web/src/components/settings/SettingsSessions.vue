<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useToast } from '@nuxt/ui/composables'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { components } from '@/api/schema'
import { useConfirm } from '@/composables/useConfirm'
import { formatMoment } from '@/utils/format'
import SectionHeader from '@/components/SectionHeader.vue'

type Session = components['schemas']['Session']

defineProps<{ embedded?: boolean }>()

const { t } = useI18n()
const sessions = ref<Session[]>([])
const error = ref<string | null>(null)
/** Starts `true`: the fetch has not run yet on the first frame, and an empty list would lie. */
const loading = ref(true)
const busyId = ref<string | null>(null)
const confirm = useConfirm()
const toast = useToast()

/// Whether "sign out everywhere else" would do anything. Offering it when there is nothing
/// else signed in invites a click that reports "0 sessions ended".
const others = computed(() => sessions.value.filter(session => !session.current).length)

onMounted(() => { void load() })

async function load(): Promise<void> {
  loading.value = true
  const response = await api.GET('/api/v1/sessions')
  loading.value = false
  if (response.data) {
    sessions.value = response.data
    error.value = null
  } else {
    error.value = responseError(response)
  }
}

async function revoke(session: Session): Promise<void> {
  const confirmed = await confirm({
    title: t('system.sessions.revoke.title'),
    description: session.current
      ? t('system.sessions.revoke.description_current')
      : t('system.sessions.revoke.description', { device: deviceLabel(session) }),
    confirmLabel: t('system.sessions.revoke.confirm'),
    confirmIcon: 'i-lucide-log-out',
    destructive: true
  })
  if (!confirmed) return
  busyId.value = session.id
  const response = await api.DELETE('/api/v1/sessions/{id}', {
    params: { path: { id: session.id } }
  })
  busyId.value = null
  if (!response.data) {
    error.value = responseError(response)
    return
  }
  // Ending your own session means the next request will not authenticate; reloading takes
  // the person to the login screen, which is the honest thing to do rather than leaving a
  // page that silently stops working.
  if (session.current) {
    window.location.reload()
    return
  }
  sessions.value = sessions.value.filter(item => item.id !== session.id)
  toast.add({ title: t('system.sessions.revoke.done'), color: 'success', icon: 'i-lucide-log-out' })
}

async function revokeOthers(): Promise<void> {
  const confirmed = await confirm({
    title: t('system.sessions.revoke_others.title'),
    description: t('system.sessions.revoke_others.description', { count: others.value }),
    confirmLabel: t('system.sessions.revoke_others.confirm'),
    confirmIcon: 'i-lucide-log-out',
    destructive: true
  })
  if (!confirmed) return
  busyId.value = 'others'
  const response = await api.POST('/api/v1/sessions/revoke-others', {})
  busyId.value = null
  if (!response.data) {
    error.value = responseError(response)
    return
  }
  await load()
  toast.add({
    title: t('system.sessions.revoke_others.done'),
    color: 'success',
    icon: 'i-lucide-log-out'
  })
}

/// A readable name for a session, from its user agent.
///
/// The full string is kept in the title attribute: a browser sends far more than anyone wants
/// in a list, but the detail is occasionally the only way to tell two similar entries apart.
function deviceLabel(session: Session): string {
  const agent = session.user_agent
  if (!agent) return t('system.sessions.unknown_device')
  const browser = ['Firefox', 'Edg', 'Chrome', 'Safari'].find(name => agent.includes(name))
  const platform = ['Windows', 'Macintosh', 'Linux', 'Android', 'iPhone', 'iPad']
    .find(name => agent.includes(name))
  if (!browser && !platform) return agent.slice(0, 40)
  const readable = browser === 'Edg' ? 'Edge' : browser
  return [readable, platform].filter(Boolean).join(' — ')
}

</script>

<template>
  <section :class="embedded ? '' : 'mt-6 border border-muted bg-default p-5'">
    <div class="flex flex-wrap items-start justify-between gap-3">
      <div>
        <SectionHeader :eyebrow="t('system.sessions.eyebrow')" :title="t('system.sessions.title')" :description="t('system.sessions.description')" />
      </div>
      <UButton
        v-if="others > 0"
        icon="i-lucide-log-out"
        color="neutral"
        variant="soft"
        :label="t('system.sessions.revoke_others.action', { count: others })"
        :loading="busyId === 'others'"
        @click="revokeOthers"
      />
    </div>

    <UAlert v-if="error" class="mt-3" color="error" variant="subtle" :description="error" />

    <div v-if="loading && sessions.length === 0" class="mt-4 text-sm text-muted">
      {{ t('system.sessions.loading') }}
    </div>
    <ul v-else class="mt-4 divide-y divide-muted border border-muted">
      <li
        v-for="session in sessions"
        :key="session.id"
        class="flex flex-wrap items-center justify-between gap-3 p-3"
      >
        <div class="min-w-0">
          <p class="flex items-center gap-2 text-sm font-medium text-highlighted">
            <span :title="session.user_agent ?? ''">{{ deviceLabel(session) }}</span>
            <UBadge v-if="session.current" color="success" variant="subtle" size="sm">
              {{ t('system.sessions.current') }}
            </UBadge>
          </p>
          <p class="mt-1 text-xs text-muted">
            {{ t('system.sessions.detail', {
              address: session.client_ip ?? t('system.sessions.unknown_address'),
              lastUsed: formatMoment(session.last_used_at),
              expires: formatMoment(session.expires_at)
            }) }}
          </p>
        </div>
        <UButton
          icon="i-lucide-log-out"
          color="neutral"
          variant="ghost"
          size="sm"
          :label="session.current ? t('system.sessions.sign_out_here') : t('system.sessions.sign_out')"
          :loading="busyId === session.id"
          @click="revoke(session)"
        />
      </li>
    </ul>
  </section>
</template>
