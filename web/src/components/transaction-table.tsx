import { Link } from 'react-router-dom'
import { formatKcoin, shortHash, timeAgo } from '../lib/format'
import type { ExplorerTransaction } from '../types'
import { AddressLink, EmptyState, StatusBadge } from './ui'

function transactionType(transaction: ExplorerTransaction): string {
  if (transaction.kind === 'claim_reward') return 'Reward'
  if (transaction.kind === 'set_display_name') return 'Display name'
  return 'Transfer'
}

export function TransactionTable({ transactions, compact = false }: { transactions: ExplorerTransaction[]; compact?: boolean }) {
  if (!transactions.length) return <EmptyState title="No transactions yet" />
  return (
    <div className="table-scroll" role="region" aria-label="Transactions" tabIndex={0}>
      <table className={`data-table transaction-table ${compact ? 'transaction-table--compact' : ''}`}>
        <thead><tr><th>ID</th><th>Type</th><th>From</th><th>To</th><th className="numeric">Amount</th><th>Status</th><th>Time</th></tr></thead>
        <tbody>
          {transactions.map((transaction) => (
            <tr key={transaction.id}>
              <td><Link className="mono table-link" to={`/transactions/${transaction.id}`}>{shortHash(transaction.id)}</Link></td>
              <td><span className="transaction-kind">{transactionType(transaction)}</span></td>
              <td>{transaction.kind === 'claim_reward' ? <span className="muted">New issuance</span> : <AddressLink address={transaction.sender} name={transaction.display_name} />}</td>
              <td>{transaction.kind === 'claim_reward' ? <AddressLink address={transaction.sender} name={transaction.display_name} /> : transaction.recipient ? <AddressLink address={transaction.recipient} /> : <span className="muted">N/A</span>}</td>
              <td className="numeric amount-cell">{transaction.kind === 'set_display_name' ? <span className="muted">N/A</span> : <>{formatKcoin(transaction.amount_atoms)} <small>KC</small></>}</td>
              <td><StatusBadge status={transaction.status === 'finalized' ? 'good' : transaction.status === 'pending' ? 'warn' : 'bad'}>{transaction.status === 'rejected' ? 'Failed' : transaction.status[0].toUpperCase() + transaction.status.slice(1)}</StatusBadge></td>
              <td className="nowrap" title={new Date(transaction.timestamp).toLocaleString()}>{timeAgo(transaction.timestamp)}</td>
            </tr>
          ))}
        </tbody>
      </table>
      {compact && <div className="table-footer"><Link to="/transactions">View all transactions</Link></div>}
    </div>
  )
}
