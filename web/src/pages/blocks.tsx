import { ArrowLeft } from 'lucide-react'
import { Link, useParams } from 'react-router-dom'
import { TransactionTable } from '../components/transaction-table'
import { CopyValue, DefinitionList, EmptyState, ErrorState, LoadingRows, PageHeading } from '../components/ui'
import { useApi } from '../lib/api-context'
import { formatInteger, shortHash, timeAgo } from '../lib/format'
import { usePaginatedResource } from '../lib/use-paginated-resource'
import { useResource } from '../lib/use-resource'
import type { ExplorerBlock } from '../types'

export function BlocksPage() {
  const { id } = useParams()
  return id ? <BlockDetail id={id} /> : <BlockList />
}

function validatorName(id: string) {
  return id.replace('validator-', 'Validator ')
}

function BlockList() {
  const { transport, historyRevision } = useApi()
  const resource = usePaginatedResource(
    (cursor, signal) => transport.blocks(cursor, signal),
    [transport],
    historyRevision,
    (block) => block.hash,
  )

  return (
    <>
      <PageHeading title="Blocks" />
      {resource.loading && !resource.initialized ? (
        <LoadingRows label="Loading blocks" />
      ) : resource.error && !resource.items.length ? (
        <ErrorState error={resource.error} retry={resource.reload} />
      ) : !resource.items.length ? (
        <EmptyState title="No finalized blocks" />
      ) : (
        <>
          <div className="table-scroll" role="region" aria-label="Finalized blocks" tabIndex={0}>
            <table className="data-table">
              <thead>
                <tr>
                  <th>Height</th>
                  <th>Block hash</th>
                  <th>Proposer</th>
                  <th>Round</th>
                  <th className="numeric">Transactions</th>
                  <th>Validator signatures</th>
                  <th>Finalized</th>
                </tr>
              </thead>
              <tbody>
                {resource.items.map((block) => {
                  const signatureCount = block.certificate?.signatures.length ?? block.signers.length
                  return (
                    <tr key={block.hash}>
                      <td>
                        <Link className="table-link table-link--strong" to={`/blocks/${block.height}`}>
                          {formatInteger(block.height)}
                        </Link>
                      </td>
                      <td>
                        <Link className="mono table-link" to={`/blocks/${block.height}`} title={block.hash}>
                          {shortHash(block.hash, 12, 7)}
                        </Link>
                      </td>
                      <td>{validatorName(block.proposer)}</td>
                      <td>{block.round}</td>
                      <td className="numeric">{block.transaction_count}</td>
                      <td>{signatureCount} of 4</td>
                      <td className="nowrap" title={new Date(block.timestamp).toLocaleString()}>{timeAgo(block.timestamp)}</td>
                    </tr>
                  )
                })}
              </tbody>
            </table>
          </div>
          {(resource.hasMore || resource.error) && (
            <div className="table-footer">
              {resource.error && <span role="alert">{resource.error.message}</span>}
              {resource.hasMore && (
                <button className="button button--secondary" type="button" onClick={resource.loadMore} disabled={resource.loadingMore}>
                  {resource.loadingMore ? 'Loading...' : 'Load older blocks'}
                </button>
              )}
            </div>
          )}
        </>
      )}
    </>
  )
}

function BlockDetail({ id }: { id: string }) {
  const { transport, historyRevision, mode } = useApi()
  const resource = useResource((signal) => transport.block(id, signal), [transport, id, historyRevision])
  const block = resource.data
  const signatureCount = block ? block.certificate?.signatures.length ?? block.signers.length : 0

  return (
    <>
      <Link className="back-link" to="/blocks"><ArrowLeft size={14} />Blocks</Link>
      <PageHeading
        title={block ? `Block ${formatInteger(block.height)}` : 'Block'}
        action={block && <span className="page-meta">Finalized, {signatureCount}/4 signatures</span>}
      />
      {resource.loading && !block ? (
        <LoadingRows label="Loading block" />
      ) : resource.error ? (
        <ErrorState error={resource.error} retry={resource.reload} />
      ) : block && (
        <>
          <section aria-labelledby="block-details-heading">
            <div className="section-heading"><h2 id="block-details-heading">Block details</h2></div>
            <div className="detail-grid">
              <DefinitionList rows={[
                { label: 'Height', value: formatInteger(block.height) },
                { label: 'Block hash', value: <CopyValue value={block.hash} compact /> },
                { label: 'Parent hash', value: <CopyValue value={block.parent_hash} compact /> },
                { label: 'Finalized', value: new Date(block.timestamp).toLocaleString() },
                { label: 'Proposer', value: validatorName(block.proposer) },
                { label: 'Round', value: block.round },
              ]} />
              <DefinitionList rows={[
                { label: 'Transactions', value: block.transaction_count },
                { label: 'Transaction root', value: <CopyValue value={block.transaction_root} compact /> },
                { label: 'State root', value: <CopyValue value={block.state_root} compact /> },
                { label: 'Header slot', value: `${validatorName(block.header_proposer)}, round ${block.header_round}` },
                { label: 'Commit certificate', value: `${signatureCount} validator signatures` },
                { label: 'Signers', value: block.signers.map((signer) => signer.replace('validator-', 'V')).join(', ') },
              ]} />
            </div>
          </section>

          <CommitCertificatePanel block={block} demo={mode === 'demo'} />

          <section className="detail-section" aria-labelledby="block-transactions-heading">
            <div className="section-heading"><h2 id="block-transactions-heading">Transactions</h2></div>
            <TransactionTable transactions={block.transactions ?? []} />
          </section>
        </>
      )}
    </>
  )
}

export function CommitCertificatePanel({ block, demo = false }: { block: ExplorerBlock; demo?: boolean }) {
  const certificate = block.certificate
  const signatureCount = certificate?.signatures.length ?? block.signers.length

  return (
    <details className="certificate-disclosure">
      <summary>
        <strong aria-hidden="true">Commit certificate</strong>
        <span className="sr-only">Commit certificate statement</span>
        <span>{signatureCount} of 4 signatures</span>
      </summary>
      {certificate ? (
        <div className="certificate-disclosure__body">
          <div>
            {demo && <p className="certificate-fixture-note">Demo mode uses deterministic certificate bytes.</p>}
            <DefinitionList rows={[
              { label: 'Chain ID', value: <span className="mono">{certificate.chain_id}</span> },
              { label: 'Height', value: formatInteger(certificate.height) },
              { label: 'Round', value: `Round ${certificate.round}` },
              { label: 'Value hash', value: <CopyValue value={certificate.consensus_value_hash} compact /> },
            ]} />
          </div>
          <div className="table-scroll certificate-signatures" role="region" aria-label="Validator signatures" tabIndex={0}>
            <table className="data-table">
              <caption className="sr-only">Validator identities and raw commit signature bytes</caption>
              <thead><tr><th>Validator</th><th>Signature bytes</th></tr></thead>
              <tbody>
                {certificate.signatures.map((signature) => (
                  <tr key={`${signature.validator}-${signature.signature}`}>
                    <td><span className="mono" title={signature.validator}>{shortHash(signature.validator, 12, 7)}</span></td>
                    <td><CopyValue value={signature.signature} compact /></td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      ) : (
        <div className="certificate-legacy-note">
          <strong>Signers reported by node</strong>
          <p>Signature bytes are unavailable in this node response. Update the node and refresh to inspect them.</p>
        </div>
      )}
    </details>
  )
}
