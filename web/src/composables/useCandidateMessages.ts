import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { LinkCandidate } from '@/api/types'
import { translateServerMessage } from '@/i18n/server'
import { hosterOf } from '@/utils/collectorSort'

/** What a LinkGrabber row says about its link's state, in the reader's language. */
export function useCandidateMessages(candidate: () => LinkCandidate) {
  const { t } = useI18n()
  /**
   * The values a coded candidate message interpolates.
   *
   * The host is a parameter and never a sentence the backend assembled: it is the candidate's
   * own address, which the row already has, so `collector.check_no_resolver` can name it in four
   * languages without a column to carry it (RD-120-18). Everything else the message needs — the
   * cause in `{detail}` of `media.ytdlp_failed` — the row stores with it (RD-1250-03); a row
   * without parameters keeps the server's English text where the sentence would have a hole.
   */
  const messageParams = computed(() => ({
    ...candidate().error_params,
    host: hosterOf(candidate())
  }))
  /**
   * What the state badge says beyond its one word.
   *
   * `duplicate` was the only state that named no reference: two links of two hosters carrying it
   * looked like a collision in the dedup key, while the key is the full address compared over
   * the whole table and both had simply been added before (RD-109-43). `unresolvable` needs the
   * same courtesy for the opposite reason: it is the one state the row offers no way out of, so
   * it has to say why rather than leave the greyed-out checkbox to be guessed at (RD-110-07).
   */
  const stateHint = computed(() => {
    if (candidate().state === 'duplicate') return t('linkgrabber.candidate.duplicate_hint')
    if (candidate().state !== 'unresolvable') return undefined
    // Two reasons end in one state, and they send the reader to opposite ends (RD-120-18): the
    // address really serves no file, or no installed plugin claims its host. The backend told
    // them apart, so the hint has to repeat that rather than blame the address in both cases.
    return candidate().error_code === 'collector.check_no_resolver'
      ? t('linkgrabber.candidate.no_resolver_hint', messageParams.value)
      : t('linkgrabber.candidate.unresolvable_hint')
  })
  /**
   * The check's message. A stable code the catalogue answers, or — for a row written before the
   * codes existed, or a sentence a plugin worded itself — the English text that came with it.
   */
  const candidateError = computed(() => {
    const { error, error_code: code } = candidate()
    return error ? translateServerMessage({ message: error, code: code ?? null, params: messageParams.value }) : ''
  })

  return { messageParams, stateHint, candidateError }
}
