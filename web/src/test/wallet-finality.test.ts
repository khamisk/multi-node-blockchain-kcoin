import { describe, expect, it } from 'vitest'
import { hasFinalizedTransaction } from '../lib/wallet'
import type { ExplorerTransaction } from '../types'

const transaction = (id: string, nonce: string, status: ExplorerTransaction['status'] = 'finalized'): ExplorerTransaction => ({
  id,
  nonce,
  status,
  kind: 'transfer',
  sender: 'kcoin1sender',
  recipient: 'kcoin1recipient',
  amount_atoms: '1',
  timestamp: '2026-07-16T00:00:00.000Z',
})

describe('wallet finality tracking', () => {
  it('marks an accepted action final only when its exact ID is in finalized history', () => {
    expect(hasFinalizedTransaction('submitted', [])).toBe(false)
    expect(hasFinalizedTransaction('submitted', [transaction('submitted', '7', 'pending')])).toBe(false)
    expect(hasFinalizedTransaction('submitted', [transaction('submitted', '7')])).toBe(true)
  })

  it('does not mistake a competing transaction with the same nonce for the submission', () => {
    const canonicalHistory = [transaction('competing', '7')]
    expect(hasFinalizedTransaction('submitted', canonicalHistory)).toBe(false)
  })
})
