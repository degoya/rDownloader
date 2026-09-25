<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api } from '@/api/client'
import AppSignature from '@/components/AppSignature.vue'
import SettingsMcpAccess from '@/components/settings/SettingsMcpAccess.vue'
import WizardPairingStep from '@/components/wizard/WizardPairingStep.vue'
import WizardPasswordStep from '@/components/wizard/WizardPasswordStep.vue'
import WizardServicesStep from '@/components/wizard/WizardServicesStep.vue'
import WizardStorageStep from '@/components/wizard/WizardStorageStep.vue'
import { useSessionStore } from '@/stores/session'

type StepKey = 'password' | 'pairing' | 'storage' | 'mcp' | 'services'

const { t } = useI18n()
const session = useSessionStore()
const endpoint = window.location.host

const STEP_ORDER: StepKey[] = ['password', 'pairing', 'storage', 'mcp', 'services']
// Re-run starts at the top so every step is reviewable; a first run resumes at the first
// step that still has work — the password is already set when the wizard was abandoned.
const current = ref<StepKey>(session.wizardRerun || session.setupRequired ? 'password' : 'pairing')
const storageStep = ref<InstanceType<typeof WizardStorageStep> | null>(null)
const advancing = ref(false)
const finishing = ref(false)
const counts = ref({ storage_roots: 0, capture_agents: 0, accounts: 0, usenet_servers: 0 })

const items = computed(() => [
  {
    value: 'password',
    title: t('wizard.steps.password.title'),
    description: t('wizard.steps.password.description'),
    icon: 'i-lucide-key-round',
    // Only the mandatory steps may block progress; in a re-run everything is freely navigable.
    disabled: !session.wizardRerun
  },
  {
    value: 'pairing',
    title: t('wizard.steps.pairing.title'),
    description: t('wizard.steps.pairing.description'),
    icon: 'i-lucide-scan-line',
    disabled: !session.wizardRerun
  },
  {
    value: 'storage',
    title: t('wizard.steps.storage.title'),
    description: t('wizard.steps.storage.description'),
    icon: 'i-lucide-hard-drive',
    disabled: !session.wizardRerun
  },
  {
    value: 'mcp',
    title: t('wizard.steps.mcp.title'),
    description: t('wizard.steps.mcp.description'),
    icon: 'i-lucide-bot',
    disabled: !session.wizardRerun
  },
  {
    value: 'services',
    title: t('wizard.steps.services.title'),
    description: t('wizard.steps.services.description'),
    icon: 'i-lucide-globe',
    disabled: !session.wizardRerun
  }
])

const index = computed(() => STEP_ORDER.indexOf(current.value))
const isLast = computed(() => index.value === STEP_ORDER.length - 1)
const isFirst = computed(() => index.value === 0)

/** The password form owns its own submit button while the password still has to be set. */
const passwordPending = computed(() => current.value === 'password' && session.setupRequired)
/** Storage is mandatory: no way forward until a root exists. */
const blocked = computed(() => current.value === 'storage' && !storageStep.value?.complete)
const skippable = computed(() => ['pairing', 'mcp', 'services'].includes(current.value))

onMounted(() => void loadCounts())

async function loadCounts(): Promise<void> {
  // On a first run the password step still has to authenticate; the counts are fetched again
  // from `passwordDone` once there is a session.
  if (!session.authenticated) return
  const response = await api.GET('/api/v1/setup/status')
  if (!response.data) return
  counts.value = {
    storage_roots: response.data.storage_roots,
    capture_agents: response.data.capture_agents,
    accounts: response.data.accounts,
    usenet_servers: response.data.usenet_servers
  }
}

async function next(): Promise<void> {
  if (blocked.value) return
  advancing.value = true
  if (current.value === 'storage') await storageStep.value?.ensureDefaultCategory()
  advancing.value = false
  const target = STEP_ORDER[index.value + 1]
  if (target) current.value = target
}

function back(): void {
  const target = STEP_ORDER[index.value - 1]
  if (target) current.value = target
}

function passwordDone(): void {
  void loadCounts()
  current.value = 'pairing'
}

async function finish(startTour: boolean): Promise<void> {
  finishing.value = true
  await api.POST('/api/v1/setup/complete')
  finishing.value = false
  session.finishWizard(startTour)
}
</script>

<template>
  <main class="signal-grid grid min-h-screen place-items-center p-5">
    <section class="w-full max-w-4xl border border-muted bg-default/95 shadow-2xl shadow-primary/5">
      <div class="h-1 transfer-stripe" />
      <div class="p-6 sm:p-9">
        <header class="mb-8 flex items-start justify-between gap-4">
          <div class="flex items-center gap-3">
            <img src="/favicon.svg" alt="rDownloader" class="size-11" />
            <div>
              <p class="eyebrow">{{ t('auth.endpoint', { endpoint }) }}</p>
              <h1 class="text-xl font-semibold tracking-tight text-highlighted">{{ t('wizard.title') }}</h1>
            </div>
          </div>
          <UButton
            v-if="session.wizardRerun"
            icon="i-lucide-x"
            color="neutral"
            variant="ghost"
            :aria-label="t('wizard.actions.close')"
            @click="session.finishWizard(false)"
          />
        </header>

        <UStepper
          v-model="current"
          :items="items"
          :linear="!session.wizardRerun"
          size="sm"
          class="mb-8"
        />

        <div class="min-h-[18rem]">
          <p class="mb-1 text-lg font-semibold text-highlighted">{{ items[index]?.title }}</p>
          <p class="mb-5 text-sm leading-6 text-muted">{{ t(`wizard.steps.${current}.lead`) }}</p>

          <WizardPasswordStep v-if="current === 'password'" @done="passwordDone" />
          <WizardPairingStep v-else-if="current === 'pairing'" />
          <WizardStorageStep v-else-if="current === 'storage'" ref="storageStep" />
          <SettingsMcpAccess v-else-if="current === 'mcp'" embedded />
          <WizardServicesStep v-else />
        </div>

        <footer class="mt-8 flex flex-wrap items-center justify-between gap-3 border-t border-muted pt-5">
          <!-- Not merely disabled on the first step: a "Back" with nowhere to go read as an
               offer (RD-120-48). `ms-auto` keeps the forward actions on the right without it. -->
          <UButton
            v-if="!isFirst"
            icon="i-lucide-arrow-left"
            color="neutral"
            variant="ghost"
            :label="t('wizard.actions.back')"
            @click="back"
          />
          <div class="ms-auto flex flex-wrap items-center gap-2">
            <UButton
              v-if="skippable && !isLast"
              color="neutral"
              variant="ghost"
              :label="t('wizard.actions.skip')"
              @click="next"
            />
            <template v-if="isLast">
              <UButton
                color="neutral"
                variant="subtle"
                :label="t('wizard.actions.finish')"
                :loading="finishing"
                @click="finish(false)"
              />
              <UButton
                icon="i-lucide-footprints"
                :label="t('wizard.actions.finish_with_tour')"
                :loading="finishing"
                @click="finish(true)"
              />
            </template>
            <UButton
              v-else-if="!passwordPending"
              icon="i-lucide-arrow-right"
              trailing
              :label="t('wizard.actions.continue')"
              :loading="advancing"
              :disabled="blocked"
              @click="next"
            />
          </div>
        </footer>
      </div>
      <AppSignature />
    </section>
  </main>
</template>
