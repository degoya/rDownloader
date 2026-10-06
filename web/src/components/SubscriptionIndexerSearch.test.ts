/**
 * An emptied age limit is sent as no limit (RD-1110-10, RD-1120-09).
 *
 * The number field hands an emptied field `undefined`; the request says "no limit" with `null`
 * (`formBody`), never by dropping the key.
 */
import { fireEvent, screen } from '@testing-library/vue'
import { describe, expect, it, vi } from 'vitest'
import { reactive } from 'vue'

import subscriptions from '@/locales/en/subscriptions.json'
import { mountComponent } from '@/test/mount'
import { emptyForm, formBody } from '@/utils/subscriptionForm'

vi.mock('@/api/client', () => ({ api: { GET: vi.fn(async () => ({ data: [] })) }, responseError: () => '' }))

import SubscriptionIndexerSearch from './SubscriptionIndexerSearch.vue'

describe('the indexer search of a subscription', () => {
  it('sends an emptied age limit as null', async () => {
    const form = reactive({ ...emptyForm(), kind: 'indexer' as const })
    form.search.maxAge = 30
    mountComponent(SubscriptionIndexerSearch, {
      messages: { subscriptions },
      props: { indexerId: form.indexerId, search: form.search }
    })

    await fireEvent.update(screen.getByTestId('subscription-search-max-age'), '')

    expect(form.search.maxAge).toBeUndefined()
    expect(formBody(form, [], false).indexer_search).toHaveProperty('max_age_days', null)
  })
})
