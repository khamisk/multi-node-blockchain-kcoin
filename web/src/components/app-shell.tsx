import { Menu, X } from 'lucide-react'
import { useEffect, useRef, useState, type FormEvent, type ReactNode } from 'react'
import { NavLink, useLocation, useNavigate } from 'react-router-dom'
import { useApi } from '../lib/api-context'
import { resolveSearchQuery } from '../lib/search'
import { useResource } from '../lib/use-resource'
import { useWallet } from '../lib/wallet-context'
import { ValidatorRail } from './validator-rail'

const nav = [
  ['/', 'Overview'],
  ['/earn', 'Earn'],
  ['/blocks', 'Blocks'],
  ['/transactions', 'Transactions'],
  ['/validators', 'Validators'],
  ['/ownership', 'Ownership'],
] as const

export function AppShell({ children }: { children: ReactNode }) {
  const navigate = useNavigate()
  const location = useLocation()
  const { mode, ready, transport, statusRevision } = useApi()
  const { wallet } = useWallet()
  const networkStatus = useResource((signal) => transport.status(signal), [transport, ready, statusRevision])
  const [query, setQuery] = useState('')
  const [open, setOpen] = useState(false)
  const [ambiguousHash, setAmbiguousHash] = useState<string>()
  const [searchError, setSearchError] = useState<string>()
  const navRef = useRef<HTMLElement>(null)
  const menuRef = useRef<HTMLButtonElement>(null)

  useEffect(() => setOpen(false), [location.pathname])

  const toggleMenu = () => {
    setOpen((current) => {
      const next = !current
      if (next) window.requestAnimationFrame(() => navRef.current?.querySelector<HTMLAnchorElement>('a')?.focus())
      return next
    })
  }

  const openSearchResult = (route: string) => {
    navigate(route)
    setQuery('')
    setAmbiguousHash(undefined)
    setSearchError(undefined)
    setOpen(false)
  }

  const submit = (event: FormEvent) => {
    event.preventDefault()
    const result = resolveSearchQuery(query)
    if (result.kind === 'route') openSearchResult(result.route)
    else if (result.kind === 'ambiguous-hash') {
      setAmbiguousHash(result.hash)
      setSearchError(undefined)
    } else {
      setAmbiguousHash(undefined)
      setSearchError(result.message)
    }
  }

  const networkPresentation = mode === 'demo'
    ? { label: 'Demo data', state: 'demo' }
    : !ready || (networkStatus.loading && !networkStatus.data)
      ? { label: 'Checking network', state: 'checking' }
      : networkStatus.error
        ? { label: 'Network unavailable', state: 'unavailable' }
        : networkStatus.data?.halted
          ? { label: 'Network halted', state: 'halted' }
          : networkStatus.data?.syncing
            ? { label: 'Network syncing', state: 'syncing' }
            : { label: 'Network ready', state: 'ready' }

  return (
    <div className="app-shell">
      <a className="skip-link" href="#main-content">Skip to content</a>
      <header className="site-header">
        <div className="site-header__top">
          <NavLink to="/" className="wordmark" aria-label="KCoin overview">KCoin</NavLink>
          <nav
            ref={navRef}
            id="primary-navigation"
            className={open ? 'main-nav main-nav--open' : 'main-nav'}
            aria-label="Primary navigation"
            onKeyDown={(event) => {
              if (event.key === 'Escape') {
                setOpen(false)
                menuRef.current?.focus()
              }
            }}
          >
            {nav.map(([to, label]) => <NavLink key={to} to={to} end={to === '/'} onClick={() => setOpen(false)}>{label}</NavLink>)}
          </nav>
          <form className="global-search" role="search" onSubmit={submit}>
            <label className="sr-only" htmlFor="chain-search">Search blocks, transactions, and addresses</label>
            <input
              id="chain-search"
              value={query}
              onChange={(event) => {
                setQuery(event.target.value)
                setAmbiguousHash(undefined)
                setSearchError(undefined)
              }}
              onKeyDown={(event) => {
                if (event.key === 'Escape') {
                  setAmbiguousHash(undefined)
                  setSearchError(undefined)
                }
              }}
              placeholder="Search block, transaction, or address"
              aria-describedby={ambiguousHash || searchError ? 'chain-search-feedback' : undefined}
              autoComplete="off"
              spellCheck={false}
            />
            {ambiguousHash && (
              <div className="search-disambiguation" id="chain-search-feedback" role="status">
                <strong>Is this a block or transaction hash?</strong>
                <button type="button" onClick={() => openSearchResult(`/blocks/${ambiguousHash}`)}>Block</button>
                <button type="button" onClick={() => openSearchResult(`/transactions/${ambiguousHash}`)}>Transaction</button>
              </div>
            )}
            {searchError && <div className="search-feedback" id="chain-search-feedback" role="alert">{searchError}</div>}
          </form>
          <div className="header-actions">
            <span className={`network-mode network-mode--${networkPresentation.state}`}><span />{networkPresentation.label}</span>
            <NavLink to="/wallet" className="wallet-link" aria-label={wallet ? 'Wallet active' : 'Wallet'} title={wallet?.address}>Wallet{wallet && <span className="sr-only"> active</span>}</NavLink>
            <button ref={menuRef} className="menu-button" type="button" aria-controls="primary-navigation" aria-expanded={open} aria-label="Toggle navigation" onClick={toggleMenu}>{open ? <X /> : <Menu />}</button>
          </div>
        </div>
        {!location.pathname.startsWith('/validators') && <ValidatorRail compact />}
      </header>
      <main className="page" id="main-content">{children}</main>
    </div>
  )
}
