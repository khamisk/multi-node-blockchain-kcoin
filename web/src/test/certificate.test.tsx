import { render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { CommitCertificatePanel } from '../pages/blocks'
import type { ExplorerBlock } from '../types'

const signature = 'cd'.repeat(64)
const valueHash = 'ab'.repeat(32)
const block: ExplorerBlock = {
  height: '42',
  hash: valueHash,
  parent_hash: '00'.repeat(32),
  proposer: 'validator-1',
  round: 2,
  header_proposer: 'validator-4',
  header_round: 0,
  timestamp: new Date(0).toISOString(),
  transaction_count: '0',
  transaction_root: '11'.repeat(32),
  state_root: '22'.repeat(32),
  signers: ['validator-1'],
  certificate: {
    chain_id: 'kcoin-localnet-1',
    height: '42',
    round: 2,
    consensus_value_hash: valueHash,
    signatures: [{ validator: 'validator-1', signature }],
  },
}

describe('commit certificate disclosure', () => {
  it('renders the signed statement and actual signature bytes supplied by the API', () => {
    render(<CommitCertificatePanel block={block} />)
    expect(screen.getByText('Commit certificate statement')).toBeInTheDocument()
    expect(screen.getByText('kcoin-localnet-1')).toBeInTheDocument()
    expect(screen.getByText('Round 2')).toBeInTheDocument()
    expect(screen.getByTitle(valueHash)).toBeInTheDocument()
    expect(screen.getByTitle(signature)).toBeInTheDocument()
  })

  it('keeps older API responses understandable without inventing raw signatures', () => {
    render(<CommitCertificatePanel block={{ ...block, certificate: undefined }} />)
    expect(screen.getByText('Signers reported by node')).toBeInTheDocument()
    expect(screen.getByText(/does not include the signature bytes/i)).toBeInTheDocument()
  })
})
