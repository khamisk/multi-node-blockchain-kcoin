import { Link } from 'react-router-dom'
import { useApi } from '../lib/api-context'
import { shortHash } from '../lib/format'
import { useResource } from '../lib/use-resource'
import type { ConsensusPhase, ValidatorStatus } from '../types'
import { LoadingRows, StatusBadge } from './ui'

const phaseLabels: Record<ConsensusPhase, string> = {
  proposal: 'Proposing',
  prevote: 'Voting',
  precommit: 'Voting',
  finalized: 'Finalized',
  syncing: 'Syncing',
  offline: 'Offline',
  halted: 'Halted',
}

const phaseDetail: Record<ConsensusPhase, string> = {
  proposal: 'Proposal',
  prevote: 'Prevote',
  precommit: 'Precommit',
  finalized: 'Up to date',
  syncing: 'Verifying missed blocks',
  offline: 'Waiting to reconnect',
  halted: 'Operator action required',
}

function heightLabel(height: string): string {
  try { return BigInt(height).toLocaleString('en-US') } catch { return height }
}

function ValidatorCell({ validator, targetHeight, demo, compact, onToggle }: { validator: ValidatorStatus; targetHeight: bigint; demo: boolean; compact: boolean; onToggle: () => void }) {
  const unavailable = !validator.online || validator.phase === 'offline' || validator.phase === 'halted'
  const tone = unavailable ? 'bad' : validator.phase === 'syncing' ? 'warn' : validator.phase === 'proposal' || validator.phase === 'prevote' || validator.phase === 'precommit' ? 'info' : 'good'
  const syncProgress = Math.round(Math.min(100, Math.max(0, validator.sync_progress ?? 0)) * 10) / 10
  const currentHeight = BigInt(validator.height)
  const blocksBehind = targetHeight > currentHeight ? targetHeight - currentHeight : 0n

  return (
    <article className={`validator validator--${validator.phase} ${compact ? 'validator--compact' : ''}`} aria-label={`${validator.name}: ${phaseLabels[validator.phase]}`}>
      <div className="validator__topline">
        <strong>{validator.name}</strong>
        <StatusBadge status={tone}>{phaseLabels[validator.phase]}</StatusBadge>
      </div>
      <div className="validator__summary">
        <span>Height <strong className="mono">{heightLabel(validator.height)}</strong></span>
        <span className="validator__phase-detail">{phaseDetail[validator.phase]}</span>
      </div>
      {validator.phase === 'syncing' && (
        <div className="sync-progress">
          <div className="sync-progress__label">
            <span>{compact ? `${syncProgress}% recovered` : `Syncing block ${heightLabel(validator.height)} of ${targetHeight.toLocaleString('en-US')}`}</span>
            <span>{blocksBehind.toLocaleString('en-US')} blocks behind</span>
          </div>
          <div className="sync-progress__track" role="progressbar" aria-label={`${validator.name} recovery progress`} aria-valuemin={0} aria-valuemax={100} aria-valuenow={syncProgress}><span style={{ width: `${syncProgress}%` }} /></div>
        </div>
      )}
      {!compact && validator.phase !== 'syncing' && (
        <dl className="validator__details">
          <div><dt>Round</dt><dd>{validator.round}</dd></div>
          <div><dt>Block</dt><dd className="mono" title={validator.block_hash}>{shortHash(validator.block_hash, 10, 6)}</dd></div>
          <div><dt>State root</dt><dd className="mono" title={validator.state_root}>{shortHash(validator.state_root, 10, 6)}</dd></div>
        </dl>
      )}
      {demo && (
        <button className="validator__control" type="button" aria-label={`${validator.online ? 'Stop' : 'Restart'} ${validator.name}`} onClick={onToggle} disabled={validator.phase === 'syncing'}>
          {validator.online ? 'Stop' : 'Restart'}
        </button>
      )}
    </article>
  )
}

export function ValidatorRail({ compact = false }: { compact?: boolean }) {
  const { transport, mode, ready, statusRevision, refresh } = useApi()
  const resource = useResource((signal) => transport.status(signal), [transport, statusRevision, ready])
  const status = resource.data
  const available = status?.validators.filter((validator) => validator.online && validator.phase !== 'syncing' && validator.phase !== 'halted' && validator.phase !== 'offline').length ?? 0
  const finalityAvailable = status ? available >= 3 : undefined
  const statusDelayed = Boolean(status && resource.error)
  const targetHeight = status?.validators.reduce((highest, validator) => {
    const height = BigInt(validator.height)
    return height > highest ? height : highest
  }, 0n) ?? 0n
  const toggle = async (validator: ValidatorStatus) => {
    await transport.setValidatorOnline?.(validator.index, !validator.online)
    refresh()
  }

  const observerState = !status
    ? resource.error ? 'Observer unavailable.' : 'Observer status loading.'
    : status.halted ? 'Observer halted; writes disabled.'
      : status.syncing ? 'Observer syncing; writes disabled.'
        : 'Observer ready.'

  return (
    <section className={`validator-panel ${compact ? 'validator-panel--compact' : 'validator-panel--full'}`} aria-labelledby={compact ? 'validator-strip-title' : 'validator-page-title'}>
      <div className="validator-panel__summary">
        {compact ? <Link id="validator-strip-title" to="/validators"><strong>Validators</strong></Link> : <strong id="validator-page-title">{status ? finalityAvailable ? 'Transactions can be confirmed' : 'Transactions are paused' : 'Network status'}</strong>}
        <span>{status ? `${available}/4 available` : resource.error ? 'Status unavailable' : 'Loading status'}</span>
        <StatusBadge status={statusDelayed ? 'warn' : finalityAvailable === undefined ? resource.error ? 'bad' : 'neutral' : finalityAvailable ? 'good' : 'bad'}>
          {statusDelayed ? 'Status delayed' : finalityAvailable === undefined ? resource.error ? 'Unavailable' : 'Checking' : finalityAvailable ? 'Ready' : 'Paused'}
        </StatusBadge>
        {!compact && <small>KCoin needs any 3 of 4 validators to confirm a block.</small>}
      </div>
      <p className="sr-only" role="status" aria-live="polite">
        {observerState} {status ? `${available} of 4 validators available. ${finalityAvailable ? 'Finality available.' : 'Finality paused.'}` : ''} {statusDelayed ? 'Validator updates are delayed.' : ''}
      </p>
      {!ready || (resource.loading && !status) ? <LoadingRows label="Loading validator status" compact={compact} /> : status ? (
        <>
          {!compact && (
            <div className="validator-table-header" aria-hidden="true">
              <span>Validator</span>
              <span>Height / phase</span>
              <span className="validator-table-header__details"><span>Round</span><span>Block</span><span>State root</span></span>
              <span />
            </div>
          )}
          <div className="validator-grid">
            {status.validators.map((validator) => (
              <ValidatorCell key={validator.id} validator={validator} targetHeight={targetHeight} demo={mode === 'demo' && !compact} compact={compact} onToggle={() => void toggle(validator)} />
            ))}
          </div>
        </>
      ) : (
        <div className="rail-error" role="alert">Validator status is unavailable.</div>
      )}
    </section>
  )
}
