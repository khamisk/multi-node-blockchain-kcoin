import { ArrowLeft } from 'lucide-react'
import { useState } from 'react'
import { Link, useParams } from 'react-router-dom'
import { TransactionTable } from '../components/transaction-table'
import { AddressLink, CopyValue, DefinitionList, EmptyState, ErrorState, LoadingRows, PageHeading, StatusBadge } from '../components/ui'
import { useApi } from '../lib/api-context'
import { formatInteger, formatKcoin } from '../lib/format'
import { usePaginatedResource } from '../lib/use-paginated-resource'
import { useResource } from '../lib/use-resource'
import type { ExplorerTransaction, TransactionStatus } from '../types'

type TransactionFilter = 'all' | 'transfer' | 'claim_reward' | 'set_display_name'

const filters: Array<[TransactionFilter, string]> = [
  ['all', 'All'],
  ['transfer', 'Transfers'],
  ['claim_reward', 'Rewards'],
  ['set_display_name', 'Display names'],
]

function transactionLabel(transaction: ExplorerTransaction) {
  if (transaction.kind === 'claim_reward') return 'Reward claim'
  if (transaction.kind === 'set_display_name') return 'Display name'
  return 'Transfer'
}

function statusLabel(status: TransactionStatus) {
  if (status === 'rejected') return 'Failed'
  return status[0].toUpperCase() + status.slice(1)
}

function statusTone(status: TransactionStatus): 'good' | 'warn' | 'bad' {
  return status === 'finalized' ? 'good' : status === 'pending' ? 'warn' : 'bad'
}

export function TransactionsPage() {
  const { id } = useParams()
  return id ? <TransactionDetail id={id} /> : <TransactionList />
}

function TransactionList() {
  const { transport, historyRevision } = useApi()
  const resource = usePaginatedResource(
    (cursor, signal) => transport.transactions(cursor, signal),
    [transport],
    historyRevision,
    (transaction) => transaction.id,
  )
  const [filter, setFilter] = useState<TransactionFilter>('all')
  const filtered = resource.items.filter((transaction) => filter === 'all' || transaction.kind === filter)

  return (
    <>
      <PageHeading title="Transactions" />
      <div className="filter-bar" aria-label="Transaction type">
        {filters.map(([value, label]) => (
          <button
            key={value}
            className={`filter-button ${filter === value ? 'filter-button--active' : ''}`}
            type="button"
            aria-pressed={filter === value}
            onClick={() => setFilter(value)}
          >
            {label}
          </button>
        ))}
        <span>{filtered.length} loaded</span>
      </div>

      {resource.loading && !resource.initialized ? (
        <LoadingRows label="Loading transactions" />
      ) : resource.error && !resource.items.length ? (
        <ErrorState error={resource.error} retry={resource.reload} />
      ) : (
        <>
          {!filtered.length ? (
            <EmptyState title="No matching transactions" detail="Choose another type or load older transactions." />
          ) : (
            <TransactionTable transactions={filtered} />
          )}
          {(resource.hasMore || resource.error) && (
            <div className="table-footer">
              {resource.error && <span role="alert">{resource.error.message}</span>}
              {resource.hasMore && (
                <button className="button button--secondary" type="button" onClick={resource.loadMore} disabled={resource.loadingMore}>
                  {resource.loadingMore ? 'Loading…' : 'Load older transactions'}
                </button>
              )}
            </div>
          )}
        </>
      )}
    </>
  )
}

function TransactionDetail({ id }: { id: string }) {
  const { transport, historyRevision } = useApi()
  const resource = useResource((signal) => transport.transaction(id, signal), [transport, id, historyRevision])
  const transaction = resource.data

  return (
    <>
      <Link className="back-link" to="/transactions"><ArrowLeft size={14} />Transactions</Link>
      <PageHeading
        title={transaction ? transactionLabel(transaction) : 'Transaction'}
        action={transaction && <StatusBadge status={statusTone(transaction.status)}>{statusLabel(transaction.status)}</StatusBadge>}
      />
      {resource.loading && !transaction ? (
        <LoadingRows label="Loading transaction" />
      ) : resource.error ? (
        <ErrorState error={resource.error} retry={resource.reload} />
      ) : transaction && (
        <>
          <section aria-labelledby="transaction-details-heading">
            <div className="section-heading"><h2 id="transaction-details-heading">Transaction details</h2></div>
            <div className="detail-grid">
              <DefinitionList rows={[
                { label: 'Transaction ID', value: <CopyValue value={transaction.id} compact /> },
                { label: 'Type', value: transactionLabel(transaction) },
                { label: 'Amount', value: transaction.kind === 'set_display_name' ? '—' : `${formatKcoin(transaction.amount_atoms)} KCoin` },
                ...(transaction.kind === 'claim_reward'
                  ? [
                      { label: 'Source', value: 'New KCoin issuance' },
                      { label: 'Claimed by', value: <AddressLink address={transaction.sender} name={transaction.display_name} /> },
                    ]
                  : [
                      { label: 'Sender', value: <AddressLink address={transaction.sender} name={transaction.display_name} /> },
                      { label: 'Recipient', value: transaction.recipient ? <AddressLink address={transaction.recipient} /> : '—' },
                    ]),
                { label: 'Wallet nonce', value: transaction.nonce },
              ]} />
              <DefinitionList rows={[
                { label: 'Status', value: <StatusBadge status={statusTone(transaction.status)}>{statusLabel(transaction.status)}</StatusBadge> },
                {
                  label: 'Finalized in',
                  value: transaction.status === 'finalized' && transaction.block_height
                    ? <Link className="inline-link" to={`/blocks/${transaction.block_height}`}>Block {formatInteger(transaction.block_height)}</Link>
                    : transaction.status === 'pending' ? 'Waiting for a block' : 'Not included',
                },
                { label: 'Block hash', value: transaction.status === 'finalized' && transaction.block_hash ? <CopyValue value={transaction.block_hash} compact /> : '—' },
                { label: 'Time', value: new Date(transaction.timestamp).toLocaleString() },
                { label: 'Confirmation', value: transaction.status === 'finalized' ? '3 of 4 validator signatures' : transaction.status === 'pending' ? 'Waiting for 3 of 4 validators' : 'Rejected' },
                ...(transaction.status === 'rejected'
                  ? [{ label: 'Error code', value: <code className="rejection-code">{transaction.rejection_code ?? 'REJECTED'}</code> }]
                  : []),
              ]} />
            </div>
          </section>
          <TransactionFinalityPanel transaction={transaction} />
        </>
      )}
    </>
  )
}

export function TransactionFinalityPanel({ transaction }: { transaction: ExplorerTransaction }) {
  if (transaction.status === 'finalized') {
    return (
      <div className="integrity-note">
        <div>
          <strong>Finalized</strong>
          <p>Confirmed by 3 of 4 validators and recorded in a block.</p>
        </div>
      </div>
    )
  }

  if (transaction.status === 'pending') {
    return (
      <div className="integrity-note integrity-note--pending">
        <div>
          <strong>Pending</strong>
          <p>Submitted. Waiting for 3 of 4 validators to include it in a block.</p>
        </div>
      </div>
    )
  }

  return (
    <div className="integrity-note integrity-note--rejected">
      <div>
        <strong>Failed</strong>
        <p><code>{transaction.rejection_code ?? 'REJECTED'}</code> — not recorded. No balance changed.</p>
      </div>
    </div>
  )
}
