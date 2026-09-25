<script setup lang="ts">
/**
 * Whether the setup the wizard walks through is actually finished.
 *
 * Deliberately only the steps the wizard makes mandatory. Anything it lets you skip — pairing,
 * MCP, a hoster account — is a choice, and listing choices as though they were missing turns a
 * correctly set up system into a page full of warnings.
 */
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api } from '@/api/client'
import { useSessionStore } from '@/stores/session'
import SectionHeader from '@/components/SectionHeader.vue'

const { t } = useI18n()
const session = useSessionStore()
const status = ref<{ wizard_completed: boolean, storage_roots: number, ephemeral_storage_roots: number, categories: number } | null>(null)

/**
 * `warn` is not a fourth state of `done`. A storage root on a path the container wipes is
 * misconfigured, not missing, so it must not count towards the outstanding steps -- otherwise
 * a finished setup reads as unfinished, which is exactly what this card avoids elsewhere.
 */
type Check = { key: string, done: boolean, warn?: boolean, to?: string }

onMounted(async () => {
  const response = await api.GET('/api/v1/setup/status')
  if (response.data) status.value = response.data
})

const checks = computed<Check[]>(() => [
  {
    key: 'login',
    // Switching the login off is a decision, not an omission: on a trusted loopback setup it
    // is the documented way to run, so it counts as settled rather than as a gap.
    done: !session.setupRequired || session.loginDisabled,
    to: '/settings/system'
  },
  {
    key: 'storage',
    done: (status.value?.storage_roots ?? 0) > 0,
    warn: (status.value?.ephemeral_storage_roots ?? 0) > 0,
    to: '/settings/routing'
  },
  {
    key: 'category',
    done: (status.value?.categories ?? 0) > 0,
    to: '/settings/routing'
  },
  {
    key: 'wizard',
    done: status.value?.wizard_completed ?? false
  }
])

const outstanding = computed(() => checks.value.filter(check => !check.done).length)
</script>

<template>
  <section class="border border-muted bg-default p-5">
    <SectionHeader :eyebrow="t('system.readiness.eyebrow')" :title="t('system.readiness.title')" />
    <p class="mt-2 text-sm leading-6 text-muted">
      {{ outstanding === 0 ? t('system.readiness.complete') : t('system.readiness.outstanding', outstanding) }}
    </p>

    <ul class="mt-4 grid gap-x-8 gap-y-2 md:grid-cols-2">
      <li v-for="check in checks" :key="check.key" class="flex items-start gap-3">
        <UIcon
          :name="check.done && !check.warn ? 'i-lucide-circle-check' : 'i-lucide-circle-alert'"
          :class="check.done && !check.warn ? 'mt-0.5 size-4 shrink-0 text-success' : 'mt-0.5 size-4 shrink-0 text-warning'"
        />
        <div class="min-w-0">
          <p class="text-sm text-highlighted">{{ t(`system.readiness.checks.${check.key}.label`) }}</p>
          <p class="mt-0.5 text-xs leading-5 text-muted">
            <template v-if="check.warn">{{ t(`system.readiness.checks.${check.key}.ephemeral`) }}</template>
            <template v-else>{{ check.done ? t(`system.readiness.checks.${check.key}.done`) : t(`system.readiness.checks.${check.key}.missing`) }}</template>
          </p>
        </div>
        <ULink v-if="(!check.done || check.warn) && check.to" :to="check.to" class="ml-auto shrink-0 text-xs">
          {{ t('system.readiness.go') }}
        </ULink>
      </li>
    </ul>
  </section>
</template>
