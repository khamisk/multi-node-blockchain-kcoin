import { OwnershipMap } from '../components/ownership-map'
import { AddressLink, EmptyState, ErrorState, LoadingRows, PageHeading } from '../components/ui'
import { useApi } from '../lib/api-context'
import { formatKcoin, percentFromBps } from '../lib/format'
import { buildOwnershipVisualEntries } from '../lib/ownership'
import { useResource } from '../lib/use-resource'

function issuedPercentage(issuedAtoms: string, unissuedAtoms: string): number {
  const issued = BigInt(issuedAtoms)
  const total = issued + BigInt(unissuedAtoms)
  return total === 0n ? 0 : Number(issued * 10_000n / total) / 100
}

export function OwnershipPage() {
  const { transport, historyRevision } = useApi()
  const resource = useResource((signal) => transport.leaderboard(undefined, signal), [transport, historyRevision])
  const data = resource.data
  const visualEntries = data ? buildOwnershipVisualEntries(data.entries, data.circulating_supply_atoms) : []
  const otherHolders = visualEntries.find((entry) => entry.aggregate)
  const issued = data ? issuedPercentage(data.circulating_supply_atoms, data.unissued_supply_atoms) : 0

  return (
    <>
      <PageHeading title="Ownership" />
      {resource.loading && !data ? <LoadingRows /> : resource.error ? <ErrorState error={resource.error} retry={resource.reload} /> : data && (
        <>
          <section aria-labelledby="ownership-summary-heading">
            <div className="section-heading">
              <h2 id="ownership-summary-heading">Supply distribution</h2>
            </div>
            <dl className="definition-list" aria-label="Ownership metrics">
              <div><dt>Circulating supply</dt><dd>{formatKcoin(data.circulating_supply_atoms, 2)} KC</dd></div>
              <div><dt>Not yet issued</dt><dd>{formatKcoin(data.unissued_supply_atoms, 2)} KC</dd></div>
              <div>
                <dt>Supply issued</dt>
                <dd>
                  {issued.toFixed(1)}%
                  <div className="supply-summary__track" role="progressbar" aria-label="KCoin supply issued" aria-valuemin={0} aria-valuemax={100} aria-valuenow={issued}>
                    <span style={{ width: `${issued}%` }} />
                  </div>
                </dd>
              </div>
              <div><dt>Largest holder</dt><dd>{percentFromBps(data.concentration.top_1_bps)}</dd></div>
              <div><dt>Top 5 holders</dt><dd>{percentFromBps(data.concentration.top_5_bps)}</dd></div>
              <div><dt>Top 10 holders</dt><dd>{percentFromBps(data.concentration.top_10_bps)}</dd></div>
            </dl>
          </section>

          {visualEntries.length === 0 ? (
            <EmptyState title="No KCoin has been issued" detail="Claim a reward to add the first holder." />
          ) : <div className="ownership-workspace detail-section">
            <section aria-labelledby="ownership-map-heading">
              <div className="section-heading">
                <h2 id="ownership-map-heading">Balance map</h2>
                <span>Circle area equals balance</span>
              </div>
              <OwnershipMap entries={data.entries} circulatingSupplyAtoms={data.circulating_supply_atoms} />
            </section>

            <section aria-labelledby="leaderboard-heading">
              <div className="section-heading">
                <h2 id="leaderboard-heading">Leaderboard</h2>
              </div>
              <div className="table-scroll ownership-table" role="region" aria-label="Ownership leaderboard" tabIndex={0}>
                <table className="data-table">
                  <thead>
                    <tr><th>Rank</th><th>Wallet</th><th className="numeric">Balance</th><th className="numeric">Share</th><th className="numeric">Transactions</th></tr>
                  </thead>
                  <tbody>
                    {data.entries.slice(0, 30).map((entry) => (
                      <tr key={entry.address}>
                        <td className="rank-cell">{entry.rank}</td>
                        <td><AddressLink address={entry.address} name={entry.display_name} /></td>
                        <td className="numeric amount-cell">{formatKcoin(entry.balance_atoms)} <small>KC</small></td>
                        <td className="numeric">{percentFromBps(entry.share_bps)}</td>
                        <td className="numeric">{entry.transaction_count}</td>
                      </tr>
                    ))}
                    {otherHolders && (
                      <tr className="aggregate-row">
                        <td className="rank-cell">N/A</td>
                        <td><strong>Other holders</strong></td>
                        <td className="numeric amount-cell">{formatKcoin(otherHolders.balance_atoms)} <small>KC</small></td>
                        <td className="numeric">{percentFromBps(otherHolders.share_bps)}</td>
                        <td className="numeric">N/A</td>
                      </tr>
                    )}
                  </tbody>
                </table>
              </div>
            </section>
          </div>}
        </>
      )}
    </>
  )
}
