import { useEffect, useState, type FormEvent } from 'react'
import { Link } from 'react-router-dom'
import { CopyValue, EmptyState, ErrorState, LoadingRows, PageHeading } from '../components/ui'
import { ApiError } from '../lib/api'
import { useApi } from '../lib/api-context'
import { formatKcoin } from '../lib/format'
import { useResource } from '../lib/use-resource'
import { useWallet } from '../lib/wallet-context'
import { hasFinalizedTransaction, signTransaction } from '../lib/wallet'
import type { AddressSummary, TransactionSubmission, WalletSession } from '../types'

type ClaimStatus = 'pending' | 'finalized'

interface ClaimState {
  submitting: boolean
  error?: string
  transaction?: {
    id: string
    status: ClaimStatus
  }
}

function emptyAccount(address: string): AddressSummary {
  return {
    address,
    balance_atoms: '0',
    nonce: '0',
    transaction_count: '0',
    transactions: [],
  }
}

function validateAnswer(value: string): string | undefined {
  if (!value.trim()) return 'Enter an answer.'
  if (!/^\d+$/.test(value.trim())) return 'Enter a whole number.'

  const answer = Number(value)
  if (!Number.isSafeInteger(answer) || answer < 0 || answer > 65_535) {
    return 'Enter a whole number from 0 to 65,535.'
  }
  return undefined
}

function claimError(reason: unknown): string {
  if (reason instanceof ApiError) {
    switch (reason.code) {
      case 'STALE_CHALLENGE':
        return 'This challenge was already claimed. Try the new challenge.'
      case 'NONCE_MISMATCH':
        return 'Your wallet state changed. Try the claim again.'
      case 'EXPIRED':
        return 'The claim expired before it was accepted. Try again.'
      case 'NODE_HALTED':
        return 'Claims are paused while the observer is safety-halted.'
      case 'NODE_SYNCING':
        return 'Claims are paused while the observer catches up.'
      default:
        return `${reason.code}: ${reason.message}`
    }
  }
  return reason instanceof Error ? reason.message : 'The claim could not be submitted.'
}

export function EarnPage() {
  const { wallet, backupConfirmed } = useWallet()

  if (!wallet || !backupConfirmed) {
    return (
      <>
        <PageHeading title="Earn" />
        <section className="earn-prerequisite">
          <div>
            <h2>{wallet ? 'Finish setting up your wallet' : 'Wallet required'}</h2>
            <p>{wallet ? 'Save and confirm your wallet backup before claiming rewards.' : 'Create or import a wallet before claiming rewards.'}</p>
          </div>
          <Link className="button button--primary" to="/wallet">
            {wallet ? 'Finish wallet setup' : 'Open Wallet'}
          </Link>
        </section>
      </>
    )
  }

  return <EarnWorkspace wallet={wallet} />
}

function EarnWorkspace({ wallet }: { wallet: WalletSession }) {
  const { transport, statusRevision, historyRevision } = useApi()
  const [answer, setAnswer] = useState('')
  const [answerError, setAnswerError] = useState<string>()
  const [claim, setClaim] = useState<ClaimState>({ submitting: false })

  const loadAccount = async (signal?: AbortSignal): Promise<AddressSummary> => {
    try {
      return await transport.address(wallet.address, signal)
    } catch (reason) {
      if (reason instanceof ApiError && reason.code === 'ADDRESS_NOT_FOUND') {
        return emptyAccount(wallet.address)
      }
      throw reason
    }
  }

  const accountResource = useResource(
    (signal) => loadAccount(signal),
    [transport, wallet.address, historyRevision],
  )
  const statusResource = useResource(
    (signal) => transport.status(signal),
    [transport, statusRevision],
  )
  const challengeResource = useResource(
    (signal) => transport.challenge(signal),
    [transport, historyRevision],
  )

  const readinessMessage = statusResource.error
    ? 'Network status updates are delayed. Retry before claiming.'
    : !statusResource.data
      ? 'Checking whether the explorer node is ready.'
    : statusResource.data.halted
      ? 'Claims are paused because the explorer node detected a safety issue.'
      : statusResource.data.syncing
        ? 'Claims resume when the explorer node finishes catching up.'
        : undefined

  const latestContext = async (): Promise<{
    account: AddressSummary
    chainId: string
    protocolVersion: number
    height: bigint
  }> => {
    const [account, status] = await Promise.all([loadAccount(), transport.status()])
    if (status.halted) {
      throw new ApiError('NODE_HALTED', 'The observer is safety-halted and cannot accept transactions.')
    }
    if (status.syncing) {
      throw new ApiError('NODE_SYNCING', 'The observer is still verifying finalized history.')
    }
    return {
      account,
      chainId: status.chain_id,
      protocolVersion: status.protocol_version,
      height: BigInt(status.height),
    }
  }

  const submitClaim = async (challengeId: string, numericAnswer: number) => {
    setClaim({ submitting: true })
    try {
      const context = await latestContext()
      const action: TransactionSubmission['action'] = {
        type: 'claim_reward',
        challenge_id: challengeId,
        answer: numericAnswer.toString(),
      }
      const unsigned: Omit<TransactionSubmission, 'signature'> = {
        protocol_version: context.protocolVersion,
        chain_id: context.chainId,
        sender_public_key: wallet.publicKey,
        nonce: context.account.nonce,
        expiry_height: (context.height + 20n).toString(),
        action,
      }
      const result = await transport.submit(await signTransaction(wallet, unsigned))
      if (result.status === 'rejected') {
        setClaim({ submitting: false, error: 'The claim was rejected before finalization.' })
        return
      }

      // Submission acceptance is not finality. Only canonical address history for
      // this exact transaction ID can move the claim to the finalized state.
      setClaim({
        submitting: false,
        transaction: { id: result.transaction_id, status: 'pending' },
      })
      setAnswer('')
      accountResource.reload()
    } catch (reason) {
      setClaim({ submitting: false, error: claimError(reason) })
      if (reason instanceof ApiError && reason.code === 'STALE_CHALLENGE') {
        challengeResource.reload()
      }
    }
  }

  const onAnswerChange = (value: string) => {
    setAnswer(value)
    if (answerError) setAnswerError(validateAnswer(value))
    if (claim.error) setClaim((current) => ({ ...current, error: undefined }))
  }

  const onSubmit = (event: FormEvent) => {
    event.preventDefault()
    const validationError = validateAnswer(answer)
    setAnswerError(validationError)
    if (validationError || !challengeResource.data) return

    void submitClaim(challengeResource.data.challenge_id, Number(answer.trim()))
  }

  const account = accountResource.data

  useEffect(() => {
    if (claim.transaction?.status !== 'pending' || !account) return
    if (!hasFinalizedTransaction(claim.transaction.id, account.transactions)) return

    setClaim((current) => current.transaction
      ? {
          submitting: false,
          transaction: { ...current.transaction, status: 'finalized' },
        }
      : current)
  }, [account, claim.transaction])

  return (
    <>
      <PageHeading
        title="Earn"
        action={
          <div className="earn-balance">
            <span>Balance</span>
            <strong>{account ? formatKcoin(account.balance_atoms) : '--'} KC</strong>
          </div>
        }
      />

      <section className="earn-account" aria-label="Active wallet">
        <span>Wallet</span>
        <CopyValue value={wallet.address} compact />
      </section>

      {readinessMessage && !(statusResource.error && !statusResource.data) && (
        <div className="earn-readiness" role="status">
          <span>{readinessMessage}</span>
        </div>
      )}

      {statusResource.error && !statusResource.data && (
        <ErrorState error={statusResource.error} retry={statusResource.reload} />
      )}

      {accountResource.loading && !account ? (
        <LoadingRows label="Loading wallet balance" />
      ) : accountResource.error ? (
        <ErrorState error={accountResource.error} retry={accountResource.reload} />
      ) : challengeResource.loading && !challengeResource.data ? (
        <LoadingRows label="Loading challenge" />
      ) : challengeResource.error ? (
        <ErrorState error={challengeResource.error} retry={challengeResource.reload} />
      ) : challengeResource.data ? (
        <section className="earn-challenge">
          <header className="earn-challenge__header">
            <div><h2>Current challenge</h2><p>Solve the problem. The first correct claim confirmed by the network receives the reward.</p></div>
            <div className="earn-reward">
              <span>Reward</span>
              <strong>{formatKcoin(challengeResource.data.reward_atoms)} KC</strong>
            </div>
          </header>

          <div className="earn-expression" aria-label={`Challenge: ${challengeResource.data.expression}`}>
            <strong>{challengeResource.data.expression}</strong>
          </div>

          <form className="earn-form" onSubmit={onSubmit} noValidate>
            <label htmlFor="earn-answer">Answer</label>
            <div className="earn-form__controls">
              <input
                id="earn-answer"
                name="answer"
                inputMode="numeric"
                autoComplete="off"
                value={answer}
                onChange={(event) => onAnswerChange(event.target.value)}
                aria-invalid={Boolean(answerError)}
                aria-describedby={answerError ? 'earn-answer-error' : undefined}
                placeholder="Enter answer"
              />
              <button
                className="button button--primary"
                type="submit"
                disabled={claim.submitting || claim.transaction?.status === 'pending' || Boolean(readinessMessage)}
              >
                {claim.submitting ? 'Submitting...' : 'Claim reward'}
              </button>
            </div>
            {answerError && <p className="earn-field-error" id="earn-answer-error" role="alert">{answerError}</p>}
          </form>
        </section>
      ) : <EmptyState title="No challenge available" detail="A new reward challenge has not been published yet." />}

      {claim.error && (
        <div className="earn-feedback earn-feedback--error" role="alert">
          <div><strong>Claim failed</strong><p>{claim.error}</p></div>
        </div>
      )}

      {claim.transaction && (
        <div
          className={`earn-feedback earn-feedback--${claim.transaction.status}`}
          role="status"
          aria-live="polite"
        >
          <div>
            <strong>{claim.transaction.status === 'finalized' ? 'Reward finalized' : 'Claim pending'}</strong>
            <p>{claim.transaction.status === 'finalized' ? 'Your balance is updated.' : 'Waiting for 3 of 4 validators to confirm.'}</p>
          </div>
          {claim.transaction.status === 'finalized' && (
            <Link to={`/transactions/${claim.transaction.id}`}>
              View transaction
            </Link>
          )}
        </div>
      )}
    </>
  )
}
