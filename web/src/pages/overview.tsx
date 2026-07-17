import { Link } from 'react-router-dom'
import { TransactionTable } from '../components/transaction-table'
import { EmptyState, ErrorState, LoadingRows, PageHeading } from '../components/ui'
import { useApi } from '../lib/api-context'
import { formatInteger, formatKcoin, shortHash, timeAgo } from '../lib/format'
import { useResource } from '../lib/use-resource'
import { useWallet } from '../lib/wallet-context'

export function OverviewPage() {
  const { transport, statusRevision, historyRevision } = useApi()
  const { wallet, backupConfirmed } = useWallet()
  const walletReady = Boolean(wallet && backupConfirmed)
  const status = useResource((signal) => transport.status(signal), [transport, statusRevision])
  const blocks = useResource((signal) => transport.blocks(undefined, signal), [transport, historyRevision])
  const transactions = useResource((signal) => transport.transactions(undefined, signal), [transport, historyRevision])
  const challenge = useResource((signal) => transport.challenge(signal), [transport, historyRevision])
  const recentBlocks = blocks.data?.items.slice(0, 5) ?? []

  return (
    <>
      <PageHeading title="Overview" />

      <section className="overview-section" aria-labelledby="overview-earn-heading">
        <div className="section-heading">
          <h2 id="overview-earn-heading">Earn</h2>
          {challenge.data && (
            <Link className="button button--primary" to={walletReady ? '/earn' : '/wallet'}>
              {walletReady ? 'Earn KCoin' : wallet ? 'Finish wallet' : 'Create wallet'}
            </Link>
          )}
        </div>
        {challenge.loading && !challenge.data ? <LoadingRows label="Loading challenge" compact /> : challenge.error && !challenge.data ? (
          <ErrorState error={challenge.error} retry={challenge.reload} />
        ) : challenge.data ? (
          <dl className="definition-list">
            <div>
              <dt>Challenge</dt>
              <dd className="mono">{challenge.data.expression}</dd>
            </div>
            <div>
              <dt>Reward</dt>
              <dd>{formatKcoin(challenge.data.reward_atoms)} KC</dd>
            </div>
          </dl>
        ) : <EmptyState title="No active challenge" />}
      </section>

      <section className="overview-section" aria-labelledby="network-heading" aria-busy={status.loading}>
        <div className="section-heading">
          <h2 id="network-heading">Network</h2>
        </div>
        {status.error && !status.data ? <ErrorState error={status.error} retry={status.reload} /> : status.data ? (
          <dl className="definition-list">
            <div><dt>Latest finalized block</dt><dd className="mono">{formatInteger(status.data.height)}</dd></div>
            <div><dt>Block hash</dt><dd className="mono">{shortHash(status.data.finalized_hash)}</dd></div>
            <div>
              <dt>Circulating supply</dt>
              <dd>{formatKcoin(status.data.circulating_supply_atoms, 2)} KC / 100,000 KC</dd>
            </div>
            <div>
              <dt>Pending transactions</dt>
              <dd>{status.data.mempool_size}</dd>
            </div>
            <div>
              <dt>Connected peers</dt>
              <dd>{status.data.peer_count}</dd>
            </div>
          </dl>
        ) : <LoadingRows label="Loading network" compact />}
      </section>

      <section className="overview-section" aria-labelledby="latest-transactions-heading">
        <div className="section-heading">
          <h2 id="latest-transactions-heading">Latest transactions</h2>
        </div>
        {transactions.loading && !transactions.data ? <LoadingRows /> : transactions.error ? (
          <ErrorState error={transactions.error} retry={transactions.reload} />
        ) : <TransactionTable transactions={transactions.data?.items.slice(0, 6) ?? []} compact />}
      </section>

      <section className="overview-section" aria-labelledby="recent-blocks-heading">
        <div className="section-heading">
          <h2 id="recent-blocks-heading">Recent blocks</h2>
          <Link to="/blocks">View all</Link>
        </div>
        {blocks.loading && !blocks.data ? <LoadingRows /> : blocks.error ? (
          <ErrorState error={blocks.error} retry={blocks.reload} />
        ) : recentBlocks.length ? (
          <div className="table-scroll" role="region" aria-label="Recent blocks" tabIndex={0}>
            <table className="data-table">
              <thead>
                <tr><th>Height</th><th>Hash</th><th>Proposer</th><th className="numeric">Transactions</th><th>Signatures</th><th>Time</th></tr>
              </thead>
              <tbody>
                {recentBlocks.map((block) => (
                  <tr key={block.hash}>
                    <td><Link className="mono table-link" to={`/blocks/${block.height}`}>{formatInteger(block.height)}</Link></td>
                    <td><Link className="mono table-link" to={`/blocks/${block.height}`}>{shortHash(block.hash, 12, 7)}</Link></td>
                    <td>{block.proposer.replace('validator-', 'Validator ')}</td>
                    <td className="numeric">{block.transaction_count}</td>
                    <td>{block.signers.length} / 4</td>
                    <td className="nowrap" title={new Date(block.timestamp).toLocaleString()}>{timeAgo(block.timestamp)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        ) : <EmptyState title="No blocks yet" detail="Finalized blocks will appear here." />}
      </section>
    </>
  )
}
