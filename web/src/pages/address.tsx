import { Navigate, useParams } from 'react-router-dom'
import { TransactionTable } from '../components/transaction-table'
import { CopyValue, DefinitionList, ErrorState, LoadingRows, Metric, PageHeading } from '../components/ui'
import { useApi } from '../lib/api-context'
import { formatInteger, formatKcoin } from '../lib/format'
import { useResource } from '../lib/use-resource'

export function AddressPage() {
  const { address } = useParams()
  const { transport, historyRevision } = useApi()
  const resource = useResource(
    (signal) => address ? transport.address(address, signal) : Promise.reject(new Error('Address is required')),
    [transport, address, historyRevision],
  )

  if (!address) return <Navigate to="/" replace />

  const account = resource.data
  const received = account?.transactions
    .filter((transaction) => transaction.recipient === address)
    .reduce((sum, transaction) => sum + BigInt(transaction.amount_atoms), 0n) ?? 0n
  const sent = account?.transactions
    .filter((transaction) => transaction.kind === 'transfer' && transaction.sender === address)
    .reduce((sum, transaction) => sum + BigInt(transaction.amount_atoms), 0n) ?? 0n

  return (
    <>
      <PageHeading title={account?.display_name || 'Address'} />
      {resource.loading && !account ? (
        <LoadingRows label="Loading address" />
      ) : resource.error ? (
        <ErrorState error={resource.error} retry={resource.reload} />
      ) : account && (
        <>
          <div className="address-banner">
            <span>Address</span>
            <CopyValue value={account.address} />
          </div>

          <section className="metrics-grid" aria-label="Address summary">
            <Metric label="Balance" value={`${formatKcoin(account.balance_atoms)} KC`} />
            <Metric label="Received" value={`${formatKcoin(received)} KC`} detail="Loaded history" />
            <Metric label="Sent" value={`${formatKcoin(sent)} KC`} detail="Loaded history" />
            <Metric label="Transactions" value={formatInteger(account.transaction_count)} detail="Finalized" />
          </section>

          <section aria-labelledby="account-details-heading">
            <div className="section-heading"><h2 id="account-details-heading">Account details</h2></div>
            <div className="detail-grid detail-grid--compact">
              <DefinitionList rows={[
                { label: 'Display name', value: account.display_name || 'Not set' },
                { label: 'Next nonce', value: account.nonce },
              ]} />
              <DefinitionList rows={[
                { label: 'Transactions', value: formatInteger(account.transaction_count) },
                { label: 'First seen', value: account.first_seen_height ? `Block ${formatInteger(account.first_seen_height)}` : 'Not yet included' },
              ]} />
            </div>
          </section>

          <section className="detail-section" aria-labelledby="address-activity-heading">
            <div className="section-heading"><h2 id="address-activity-heading">Activity</h2></div>
            <TransactionTable transactions={account.transactions} />
          </section>
        </>
      )}
    </>
  )
}
