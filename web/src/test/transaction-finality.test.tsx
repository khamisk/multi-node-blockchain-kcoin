import { render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { TransactionFinalityPanel } from '../pages/transactions'
import type { ExplorerTransaction } from '../types'

const base: ExplorerTransaction = {
  id: 'ab'.repeat(32),
  kind: 'transfer',
  status: 'pending',
  sender: 'kcoin1sender',
  recipient: 'kcoin1recipient',
  amount_atoms: '1000000',
  nonce: '0',
  timestamp: new Date(0).toISOString(),
}

describe('transaction finality status', () => {
  it('distinguishes pending from finalized transactions', () => {
    const { rerender } = render(<TransactionFinalityPanel transaction={{ ...base, status: 'pending' }} />)
    expect(screen.getByText('Pending')).toBeInTheDocument()
    expect(screen.queryByText('Finalized')).not.toBeInTheDocument()

    rerender(<TransactionFinalityPanel transaction={{ ...base, status: 'finalized' }} />)
    expect(screen.getByText('Finalized')).toBeInTheDocument()
  })

  it('shows the stable rejection code and confirms that no balance changed', () => {
    render(<TransactionFinalityPanel transaction={{ ...base, status: 'rejected', rejection_code: 'INVALID_SIGNATURE' }} />)
    expect(screen.getByText('Failed')).toBeInTheDocument()
    expect(screen.getByText('INVALID_SIGNATURE')).toBeInTheDocument()
    expect(screen.getByText(/no balance changed/i)).toBeInTheDocument()
  })
})
