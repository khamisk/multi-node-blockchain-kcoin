import { useEffect, useRef, useState, type FormEvent } from 'react'
import { Link } from 'react-router-dom'
import { CopyValue, ErrorState, LoadingRows, PageHeading } from '../components/ui'
import { ApiError } from '../lib/api'
import { useApi } from '../lib/api-context'
import { decodeBech32m } from '../lib/bech32'
import { formatKcoin, parseKcoin, shortHash } from '../lib/format'
import { useResource } from '../lib/use-resource'
import { useWallet } from '../lib/wallet-context'
import {
  downloadWalletBackup,
  exportWallet,
  generateWallet,
  hasFinalizedTransaction,
  importWallet,
  signTransaction,
  supportsEd25519,
} from '../lib/wallet'
import type { AddressSummary, TransactionSubmission, WalletSession } from '../types'

type ActionKind = 'transfer' | 'name'

interface ActionError {
  kind: ActionKind
  message: string
  code?: string
}

interface ActionResult {
  kind: ActionKind
  id: string
  status: 'pending' | 'finalized'
}

interface ActionState {
  pending?: ActionKind
  error?: ActionError
  result?: ActionResult
}

function readableError(kind: ActionKind, reason: unknown): ActionError {
  if (reason instanceof ApiError) {
    const messages: Record<string, string> = {
      INVALID_SIGNATURE: 'The node could not verify this wallet signature.',
      NONCE_MISMATCH: 'Your wallet state changed. Try again.',
      INSUFFICIENT_BALANCE: 'Your finalized balance is too low for this transfer.',
      EXPIRED: 'The transaction expired before it was accepted. Try again.',
      MALFORMED: 'The node could not read this transaction.',
      NODE_HALTED: 'Sending is paused while the observer is safety-halted.',
      NODE_SYNCING: 'Sending is paused while the observer catches up.',
    }
    return { kind, code: reason.code, message: messages[reason.code] ?? reason.message }
  }
  return {
    kind,
    message: reason instanceof Error ? reason.message : 'The transaction could not be submitted.',
  }
}

function validateRecipient(value: string): string | undefined {
  if (!value.trim()) return 'Enter a recipient address.'
  try {
    const bytes = decodeBech32m(value.trim())
    if (bytes.length !== 20) return 'Enter a valid KCoin address.'
  } catch {
    return 'Enter a valid kcoin1 address.'
  }
  return undefined
}

function validateAmount(value: string, balanceAtoms: string): { atoms?: string; error?: string } {
  if (!value.trim()) return { error: 'Enter an amount.' }
  try {
    const atoms = parseKcoin(value)
    if (BigInt(atoms) <= 0n) return { error: 'Amount must be greater than zero.' }
    if (BigInt(atoms) > BigInt(balanceAtoms)) return { error: 'Amount exceeds your available balance.' }
    return { atoms }
  } catch (reason) {
    return { error: reason instanceof Error ? reason.message : 'Enter a valid amount.' }
  }
}

export function WalletPage() {
  const { wallet, setWallet, backupConfirmed, confirmBackup } = useWallet()
  const [creating, setCreating] = useState(false)
  const [error, setError] = useState<string>()
  const [backupDownloaded, setBackupDownloaded] = useState(false)
  const fileRef = useRef<HTMLInputElement>(null)

  const create = async () => {
    setCreating(true)
    setError(undefined)
    try {
      const next = await generateWallet()
      setWallet(next)
      const blob = await exportWallet(next)
      downloadWalletBackup(blob, next.address)
      setBackupDownloaded(true)
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : 'Wallet creation failed.')
    } finally {
      setCreating(false)
    }
  }

  const importFile = async (file?: File) => {
    if (!file) return
    setCreating(true)
    setError(undefined)
    try {
      const next = await importWallet(file)
      setWallet(next)
      confirmBackup()
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : 'Wallet import failed.')
    } finally {
      setCreating(false)
      if (fileRef.current) fileRef.current.value = ''
    }
  }

  return (
    <>
      <PageHeading title="Wallet" />

      {!supportsEd25519() ? (
        <div className="message-state message-state--error" role="alert">
          <div><strong>This browser cannot create a wallet</strong><p>Use a current version of Chrome, Firefox, or Safari.</p></div>
        </div>
      ) : !wallet ? (
        <section className="wallet-entry" aria-label="Wallet setup">
          <div className="wallet-entry__actions button-row">
            <button className="button button--primary" type="button" onClick={() => void create()} disabled={creating}>
              {creating ? 'Creating...' : 'Create wallet'}
            </button>
            <label className="button button--secondary file-button" aria-disabled={creating}>
              Import wallet
              <input ref={fileRef} type="file" accept="application/json,.json" disabled={creating} onChange={(event) => void importFile(event.target.files?.[0])} />
            </label>
          </div>
          {error && <div className="form-error wallet-entry__error" role="alert">{error}</div>}
        </section>
      ) : !backupConfirmed ? (
        <section className="backup-gate">
          <div className="backup-gate__heading"><h2>Back up your wallet</h2><p>Download this file and keep it private. You need it to import the wallet again.</p></div>
          <div className="backup-address"><span>Wallet address</span><CopyValue value={wallet.address} /></div>
          <div className="button-row">
            <button
              className="button button--secondary"
              type="button"
              onClick={async () => {
                downloadWalletBackup(await exportWallet(wallet), wallet.address)
                setBackupDownloaded(true)
              }}
            >
              Download backup
            </button>
            <button className="button button--primary" type="button" disabled={!backupDownloaded} onClick={confirmBackup}>
              I saved the backup
            </button>
          </div>
        </section>
      ) : (
        <WalletWorkspace wallet={wallet} />
      )}
    </>
  )
}

function WalletWorkspace({ wallet }: { wallet: WalletSession }) {
  const { transport, statusRevision, historyRevision, mode } = useApi()
  const { lock, updateDisplayName } = useWallet()
  const [recipient, setRecipient] = useState('')
  const [amount, setAmount] = useState('')
  const [recipientError, setRecipientError] = useState<string>()
  const [amountError, setAmountError] = useState<string>()
  const [name, setName] = useState(wallet.displayName)
  const [nameError, setNameError] = useState<string>()
  const [action, setAction] = useState<ActionState>({})

  const emptyAccount = (): AddressSummary => ({
    address: wallet.address,
    balance_atoms: '0',
    nonce: '0',
    transaction_count: '0',
    transactions: [],
  })

  const loadAccount = async (signal?: AbortSignal): Promise<AddressSummary> => {
    try {
      return await transport.address(wallet.address, signal)
    } catch (reason) {
      if (reason instanceof ApiError && reason.code === 'ADDRESS_NOT_FOUND') return emptyAccount()
      throw reason
    }
  }

  const accountResource = useResource((signal) => loadAccount(signal), [transport, wallet.address, historyRevision])
  const statusResource = useResource((signal) => transport.status(signal), [transport, statusRevision])
  const account = accountResource.data
  const readinessMessage = statusResource.error
    ? 'Network status updates are delayed. Retry before sending.'
    : !statusResource.data
      ? 'Checking network readiness.'
    : statusResource.data.halted
      ? 'The observer is halted. Sending is disabled until the chain is verified.'
      : statusResource.data.syncing
        ? 'The observer is catching up. Sending resumes when recovery is complete.'
        : undefined
  const signingDisabled = Boolean(action.pending) || action.result?.status === 'pending' || Boolean(readinessMessage) || !account

  const latestContext = async (): Promise<{ account: AddressSummary; chainId: string; protocolVersion: number; height: bigint }> => {
    const [latestAccount, status] = await Promise.all([loadAccount(), transport.status()])
    if (status.halted) throw new ApiError('NODE_HALTED', 'The observer is safety-halted and cannot accept transactions.')
    if (status.syncing) throw new ApiError('NODE_SYNCING', 'The observer is still verifying finalized history.')
    return {
      account: latestAccount,
      chainId: status.chain_id,
      protocolVersion: status.protocol_version,
      height: BigInt(status.height),
    }
  }

  const submit = async (kind: ActionKind, txAction: TransactionSubmission['action']) => {
    setAction({ pending: kind })
    try {
      const context = await latestContext()
      const unsigned: Omit<TransactionSubmission, 'signature'> = {
        protocol_version: context.protocolVersion,
        chain_id: context.chainId,
        sender_public_key: wallet.publicKey,
        nonce: context.account.nonce,
        expiry_height: (context.height + 20n).toString(),
        action: txAction,
      }
      const result = await transport.submit(await signTransaction(wallet, unsigned))
      if (result.status === 'rejected') {
        setAction({ error: { kind, message: 'The node rejected this transaction before finalization.' } })
        return
      }
      setAction({ result: { kind, id: result.transaction_id, status: result.status } })
      if (kind === 'transfer') {
        setRecipient('')
        setAmount('')
      } else {
        updateDisplayName(name.trim())
      }
      accountResource.reload()
    } catch (reason) {
      setAction({ error: readableError(kind, reason) })
    }
  }

  const send = (event: FormEvent) => {
    event.preventDefault()
    const nextRecipientError = validateRecipient(recipient)
    const amountValidation = validateAmount(amount, account?.balance_atoms ?? '0')
    setRecipientError(nextRecipientError)
    setAmountError(amountValidation.error)
    if (nextRecipientError || amountValidation.error || !amountValidation.atoms) return
    void submit('transfer', {
      type: 'transfer',
      recipient: recipient.trim(),
      amount_atoms: amountValidation.atoms,
    })
  }

  const registerName = (event: FormEvent) => {
    event.preventDefault()
    const value = name.trim()
    const invalid = value.length > 32 || new TextEncoder().encode(value).length > 64
      ? 'Display names are limited to 32 characters and 64 UTF-8 bytes.'
      : undefined
    setNameError(invalid)
    if (invalid) return
    void submit('name', { type: 'set_display_name', display_name: value || null })
  }

  useEffect(() => {
    if (action.result?.status !== 'pending' || !account) return
    // Finality requires this exact ID in canonical address history. A competing
    // transaction with the same nonce cannot complete this action.
    if (hasFinalizedTransaction(action.result.id, account.transactions)) {
      setAction((current) => current.result
        ? { result: { ...current.result, status: 'finalized' } }
        : current)
    }
  }, [account, action.result])

  const downloadBackup = async () => {
    downloadWalletBackup(await exportWallet(wallet), wallet.address)
  }

  return (
    <>
      <section className="wallet-summary" aria-label="Active wallet">
        <div className="wallet-summary__identity">
          <strong>{account?.display_name || 'Unnamed wallet'}</strong>
          <CopyValue value={wallet.address} />
        </div>
        <dl className="wallet-summary__facts">
          <div><dt>Balance</dt><dd>{account ? formatKcoin(account.balance_atoms) : '--'} <small>KC</small></dd></div>
          <div><dt>Backup</dt><dd>Confirmed</dd></div>
          <div><dt>Next nonce</dt><dd className="mono">{account?.nonce ?? '--'}</dd></div>
        </dl>
        <button className="button button--secondary" type="button" onClick={lock} title="Clears this wallet from the tab. Import the backup to reopen it.">Close wallet</button>
      </section>

      {readinessMessage && !(statusResource.error && !statusResource.data) && (
        <div className="wallet-readiness" role="status">
          <strong>Transactions paused</strong>
          <span>{readinessMessage}</span>
        </div>
      )}

      {statusResource.error && !statusResource.data && (
        <ErrorState error={statusResource.error} retry={statusResource.reload} />
      )}

      {accountResource.loading && !account ? (
        <LoadingRows label="Loading wallet" />
      ) : accountResource.error ? (
        <ErrorState error={accountResource.error} retry={accountResource.reload} />
      ) : (
        <div className="wallet-layout">
          <section className="wallet-send">
            <header className="section-heading">
              <h2>Send KCoin</h2>
              <span>{formatKcoin(account?.balance_atoms ?? '0')} KC available</span>
            </header>
            <form className="wallet-form" onSubmit={send} noValidate>
              <label htmlFor="send-recipient">Recipient address</label>
              <input
                id="send-recipient"
                className="mono"
                value={recipient}
                onChange={(event) => {
                  setRecipient(event.target.value)
                  if (recipientError) setRecipientError(validateRecipient(event.target.value))
                  if (action.error?.kind === 'transfer') setAction({})
                }}
                placeholder="kcoin1..."
                spellCheck={false}
                autoComplete="off"
                aria-invalid={Boolean(recipientError)}
                aria-describedby={recipientError ? 'send-recipient-error' : undefined}
              />
              {recipientError && <p className="field-error" id="send-recipient-error" role="alert">{recipientError}</p>}

              <div className="field-label-row"><label htmlFor="send-amount">Amount</label><span>6 decimal places maximum</span></div>
              <div className="amount-input">
                <input
                  id="send-amount"
                  inputMode="decimal"
                  value={amount}
                  onChange={(event) => {
                    setAmount(event.target.value)
                    if (amountError) setAmountError(validateAmount(event.target.value, account?.balance_atoms ?? '0').error)
                    if (action.error?.kind === 'transfer') setAction({})
                  }}
                  placeholder="0.000000"
                  aria-invalid={Boolean(amountError)}
                  aria-describedby={amountError ? 'send-amount-error' : undefined}
                />
                <span>KC</span>
              </div>
              {amountError && <p className="field-error" id="send-amount-error" role="alert">{amountError}</p>}

              <button className="button button--primary" type="submit" disabled={signingDisabled}>
                {action.pending === 'transfer' ? 'Submitting...' : 'Send KCoin'}
              </button>
            </form>
            <ActionFeedback action={action} kind="transfer" />
          </section>

          <aside className="wallet-sidebar">
            <section className="wallet-settings">
              <header className="section-heading"><h2>Display name</h2></header>
              <form className="wallet-form" onSubmit={registerName} noValidate>
                <label htmlFor="wallet-name">Public name</label>
                <input
                  id="wallet-name"
                  value={name}
                  onChange={(event) => {
                    setName(event.target.value)
                    setNameError(undefined)
                    if (action.error?.kind === 'name') setAction({})
                  }}
                  placeholder="Optional"
                  maxLength={32}
                  aria-invalid={Boolean(nameError)}
                  aria-describedby={nameError ? 'wallet-name-error' : 'wallet-name-help'}
                />
                {nameError && <p className="field-error" id="wallet-name-error" role="alert">{nameError}</p>}
                <p className="field-help" id="wallet-name-help">Cosmetic only. Transactions still use wallet addresses.</p>
                <button className="button button--secondary" type="submit" disabled={signingDisabled}>
                  {action.pending === 'name' ? 'Saving...' : 'Save display name'}
                </button>
              </form>
              <ActionFeedback action={action} kind="name" />
            </section>

            <section className="wallet-backup-row">
              <strong>Wallet backup</strong>
              <button className="button button--secondary button--compact" type="button" onClick={() => void downloadBackup()}>Download backup</button>
            </section>
          </aside>
        </div>
      )}

      <details className="wallet-details">
        <summary>Wallet details</summary>
        <dl className="wallet-status-line" aria-label="Wallet technical status">
          <div><dt>Signing</dt><dd>{statusResource.error && !statusResource.data ? 'Unavailable' : readinessMessage ? 'Paused' : 'Ready'}</dd></div>
          <div><dt>Network</dt><dd>{mode === 'demo' ? 'Demo' : statusResource.data?.halted ? 'Observer halted' : statusResource.data?.syncing ? 'Observer syncing' : statusResource.data ? 'Observer ready' : 'Checking observer'}</dd></div>
          <div><dt>Chain</dt><dd className="mono">{statusResource.data?.chain_id ?? '--'}</dd></div>
          <div><dt>Public key</dt><dd className="mono">{shortHash(wallet.publicKey, 9, 5)}</dd></div>
        </dl>
      </details>
    </>
  )
}

function ActionFeedback({ action, kind }: { action: ActionState; kind: ActionKind }) {
  if (action.error?.kind === kind) {
    return (
      <div className="action-feedback action-feedback--error" role="alert">
        <div>
          <strong>{kind === 'transfer' ? 'Send failed' : 'Name update failed'}</strong>
          <p>{action.error.message}</p>
          {action.error.code && <code>{action.error.code}</code>}
        </div>
      </div>
    )
  }

  if (action.result?.kind !== kind) return null
  const finalized = action.result.status === 'finalized'
  return (
    <div className={`action-feedback action-feedback--${action.result.status}`} role="status" aria-live="polite">
      <div>
        <strong>{kind === 'transfer' ? `Transfer ${action.result.status}` : `Display name ${action.result.status}`}</strong>
        <p>{finalized ? 'Included in a finalized block.' : 'Waiting for 3 of 4 validators.'}</p>
      </div>
      {finalized && <Link to={`/transactions/${action.result.id}`}>View transaction</Link>}
    </div>
  )
}
