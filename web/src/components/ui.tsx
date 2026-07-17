import { Check, Copy } from 'lucide-react'
import { useState, type ReactNode } from 'react'
import { Link } from 'react-router-dom'
import { shortHash } from '../lib/format'

export function PageHeading({ title, action }: { title: string; action?: ReactNode }) {
  return (
    <header className="page-heading">
      <h1>{title}</h1>
      {action && <div className="page-heading__action">{action}</div>}
    </header>
  )
}

export function LoadingRows({ label = 'Loading chain data', compact = false }: { label?: string; compact?: boolean }) {
  return (
    <div className={`loading-rows ${compact ? 'loading-rows--compact' : ''}`} role="status" aria-label={label}>
      <span /><span /><span />
    </div>
  )
}

export function ErrorState({ error, retry }: { error: Error; retry?: () => void }) {
  return (
    <div className="message-state message-state--error" role="alert">
      <div><strong>Unable to load data</strong><p>{error.message}</p></div>
      {retry && <button className="button button--secondary" type="button" onClick={retry}>Retry</button>}
    </div>
  )
}

export function EmptyState({ title, detail }: { title: string; detail?: string }) {
  return <div className="message-state message-state--empty"><div><strong>{title}</strong>{detail && <p>{detail}</p>}</div></div>
}

export function CopyValue({ value, compact = false }: { value: string; compact?: boolean }) {
  const [state, setState] = useState<'idle' | 'copied' | 'error'>('idle')
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(value)
      setState('copied')
      window.setTimeout(() => setState('idle'), 1200)
    } catch {
      setState('error')
    }
  }
  return (
    <span className="copy-value">
      <span className="mono" title={value}>{compact ? shortHash(value) : value}</span>
      <button className="icon-button" type="button" onClick={() => void copy()} aria-label={`Copy ${value}`} title={state === 'copied' ? 'Copied' : state === 'error' ? 'Copy failed' : 'Copy'}>
        {state === 'copied' ? <Check size={14} /> : <Copy size={14} />}
      </button>
      {state === 'error' && <span className="copy-feedback" role="status">Copy failed</span>}
      <span className="sr-only" aria-live="polite">{state === 'copied' ? 'Copied' : state === 'error' ? 'Copy failed' : ''}</span>
    </span>
  )
}

export function AddressLink({ address, name, compact = true }: { address: string; name?: string; compact?: boolean }) {
  return (
    <Link className="identity inline-link" to={`/address/${address}`} title={address}>
      {name && <span className="identity__name">{name}</span>}
      <span className="mono">{compact ? shortHash(address, 10, 5) : address}</span>
    </Link>
  )
}

export function StatusDot({ status }: { status: 'good' | 'warn' | 'bad' | 'quiet' }) {
  return <span className={`status-dot status-dot--${status}`} aria-hidden="true" />
}

export function StatusBadge({ status, children }: { status: 'good' | 'warn' | 'bad' | 'neutral' | 'info'; children: ReactNode }) {
  return <span className={`status-badge status-badge--${status}`}>{children}</span>
}

export function Metric({ label, value, detail }: { label: string; value: ReactNode; detail?: ReactNode }) {
  return (
    <div className="metric">
      <span className="metric__label">{label}</span>
      <strong className="metric__value">{value}</strong>
      {detail && <span className="metric__detail">{detail}</span>}
    </div>
  )
}

export function DefinitionList({ rows }: { rows: Array<{ label: string; value: ReactNode }> }) {
  return (
    <dl className="definition-list">
      {rows.map((row) => <div key={row.label}><dt>{row.label}</dt><dd>{row.value}</dd></div>)}
    </dl>
  )
}
