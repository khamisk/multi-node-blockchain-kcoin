import type {
  AddressSummary,
  ApiEvent,
  Challenge,
  ExplorerBlock,
  ExplorerTransaction,
  ExplorerTransport,
  LeaderboardResponse,
  NetworkStatus,
  Paginated,
  SubmissionResult,
  TransactionSubmission,
  ValidatorStatus,
} from '../types'
import { encodeBech32m } from './bech32'
import { decodeBase64, deterministicHex } from './format'
import { publicKeyToAddress } from './wallet'

const K = 1_000_000n
const MAX_SUPPLY = 100_000n * K

export function evaluateDemoChallenge(expression: string): number {
  const match = expression.match(/^\s*(\d+)\s*([+−×])\s*(\d+)\s*$/u)
  if (!match) throw new Error('Demo challenge expression is malformed.')
  const left = Number(match[1])
  const right = Number(match[3])
  switch (match[2]) {
    case '+': return left + right
    case '−': return left - right
    case '×': return left * right
    default: throw new Error('Demo challenge operation is unsupported.')
  }
}

/** Mirrors kcoin_protocol::reward_for_supply, including the final partial reward. */
export function demoRewardForSupply(totalSupplyAtoms: bigint): bigint {
  if (totalSupplyAtoms >= MAX_SUPPLY) return 0n
  const band = totalSupplyAtoms / (20_000n * K)
  const rewardKcoin = [100n, 50n, 25n, 10n, 5n][Number(band)] ?? 5n
  const scheduled = rewardKcoin * K
  const remaining = MAX_SUPPLY - totalSupplyAtoms
  return scheduled < remaining ? scheduled : remaining
}

function bytesFromSeed(seed: string): Uint8Array {
  return Uint8Array.from(deterministicHex(seed, 40).match(/../g) ?? [], (pair) => Number.parseInt(pair, 16))
}

function address(seed: string): string {
  return encodeBech32m('kcoin', bytesFromSeed(seed))
}

function hash(seed: string): string {
  return deterministicHex(seed, 64)
}

function iso(minutesAgo: number): string {
  return new Date(Date.now() - minutesAgo * 60_000).toISOString()
}

const identities = [
  { address: address('ada'), display_name: 'Ada', balance: 12_480n * K, txs: 18 },
  { address: address('grace'), display_name: 'Grace', balance: 9_320n * K, txs: 13 },
  { address: address('linus'), display_name: 'Linus', balance: 7_100n * K, txs: 10 },
  { address: address('margaret'), display_name: 'Margaret', balance: 5_250n * K, txs: 8 },
  { address: address('barbara'), display_name: 'Barbara', balance: 3_400n * K, txs: 7 },
  { address: address('dennis'), display_name: 'Dennis', balance: 2_800n * K, txs: 6 },
  { address: address('radia'), display_name: 'Radia', balance: 2_100n * K, txs: 5 },
  { address: address('ken'), display_name: 'Ken', balance: 1_600n * K, txs: 4 },
  { address: address('leslie'), display_name: 'Leslie', balance: 1_100n * K, txs: 3 },
  { address: address('frances'), display_name: 'Frances', balance: 750n * K, txs: 3 },
  { address: address('mary'), display_name: 'Mary', balance: 620n * K, txs: 3 },
  { address: address('evelyn'), display_name: 'Evelyn', balance: 560n * K, txs: 2 },
  { address: address('sophie'), display_name: 'Sophie', balance: 510n * K, txs: 2 },
  { address: address('jean'), display_name: 'Jean', balance: 470n * K, txs: 2 },
  { address: address('karen'), display_name: 'Karen', balance: 420n * K, txs: 2 },
  { address: address('hedy'), display_name: 'Hedy', balance: 380n * K, txs: 2 },
  { address: address('joan'), display_name: 'Joan', balance: 340n * K, txs: 2 },
  { address: address('anita'), display_name: 'Anita', balance: 300n * K, txs: 1 },
  { address: address('adele'), display_name: 'Adele', balance: 270n * K, txs: 1 },
  { address: address('katherine'), display_name: 'Katherine', balance: 240n * K, txs: 1 },
  { address: address('annie'), display_name: 'Annie', balance: 210n * K, txs: 1 },
  { address: address('mary-allen'), display_name: 'Mary A.', balance: 185n * K, txs: 1 },
  { address: address('lois'), display_name: 'Lois', balance: 160n * K, txs: 1 },
  { address: address('irma'), display_name: 'Irma', balance: 140n * K, txs: 1 },
  { address: address('sister-mary'), display_name: 'Sister Mary', balance: 120n * K, txs: 1 },
  { address: address('mary-ross'), display_name: 'Mary R.', balance: 100n * K, txs: 1 },
  { address: address('kateryna'), display_name: 'Kateryna', balance: 85n * K, txs: 1 },
  { address: address('arfa'), display_name: 'Arfa', balance: 70n * K, txs: 1 },
  { address: address('kimberly'), display_name: 'Kimberly', balance: 55n * K, txs: 1 },
  { address: address('samaira'), display_name: 'Samaira', balance: 40n * K, txs: 1 },
  { address: address('other-a'), balance: 35n * K, txs: 1 },
  { address: address('other-b'), balance: 25n * K, txs: 1 },
  { address: address('other-c'), balance: 15n * K, txs: 1 },
  { address: address('other-d'), balance: 8n * K, txs: 1 },
  { address: address('other-e'), balance: 2n * K, txs: 1 },
]

const SEEDED_SUPPLY_ATOMS = identities.reduce((sum, identity) => sum + identity.balance, 0n)

interface DemoAccount {
  address: string
  displayName?: string
  balance: bigint
  nonce: bigint
  transactionCount: number
  firstSeenHeight: string
}

class DemoChain implements ExplorerTransport {
  private height = 1642n
  private stateRoot = hash('state-1642')
  private transactionsList: ExplorerTransaction[]
  private blocksList: ExplorerBlock[]
  private accounts = new Map<string, DemoAccount>()
  private listeners = new Set<(event: ApiEvent) => void>()
  private validatorState: ValidatorStatus[]
  private challengeValue: Challenge = {
    challenge_id: '1642',
    expression: '7 × 6',
    issued_at_height: '1642',
    reward_atoms: demoRewardForSupply(SEEDED_SUPPLY_ATOMS).toString(),
  }

  constructor() {
    identities.forEach((identity, index) => this.accounts.set(identity.address, {
      address: identity.address,
      displayName: identity.display_name,
      balance: identity.balance,
      nonce: BigInt(identity.txs),
      transactionCount: identity.txs,
      firstSeenHeight: String(1180 + index * 21),
    }))

    this.transactionsList = [
      this.transferTx('tx-a', identities[0], identities[6], 125n, 1642, 0),
      this.rewardTx('tx-b', identities[2], 25n, 1641, 1),
      this.transferTx('tx-c', identities[1], identities[3], 48n, 1640, 3),
      this.transferTx('tx-d', identities[4], identities[8], 250n, 1639, 6),
      this.rewardTx('tx-e', identities[0], 25n, 1638, 8),
      this.transferTx('tx-f', identities[3], identities[9], 32n, 1637, 12),
      this.transferTx('tx-g', identities[6], identities[5], 76n, 1636, 15),
      this.rewardTx('tx-h', identities[1], 25n, 1635, 18),
    ]

    this.blocksList = Array.from({ length: 18 }, (_, offset) => {
      const height = Number(this.height) - offset
      const transactions = this.transactionsList.filter((transaction) => transaction.block_height === String(height))
      return this.makeBlock(height, transactions, offset)
    })

    const tip = this.blocksList[0]
    this.validatorState = Array.from({ length: 4 }, (_, index) => ({
      id: `validator-${index + 1}`,
      name: `Validator ${index + 1}`,
      index,
      online: true,
      phase: index === 0 ? 'proposal' : index === 1 ? 'prevote' : 'precommit',
      height: this.height.toString(),
      round: 0,
      block_hash: tip.hash,
      state_root: tip.state_root,
      last_seen_ms: String(20 + index * 14),
    }))
  }

  private transferTx(seed: string, sender: typeof identities[number], recipient: typeof identities[number], amount: bigint, height: number, minutesAgo: number): ExplorerTransaction {
    return {
      id: hash(seed),
      kind: 'transfer',
      status: 'finalized',
      sender: sender.address,
      recipient: recipient.address,
      amount_atoms: (amount * K).toString(),
      nonce: String(sender.txs - 1),
      display_name: sender.display_name,
      block_height: String(height),
      block_hash: hash(`block-${height}`),
      timestamp: iso(minutesAgo),
    }
  }

  private rewardTx(seed: string, recipient: typeof identities[number], amount: bigint, height: number, minutesAgo: number): ExplorerTransaction {
    return {
      id: hash(seed),
      kind: 'claim_reward',
      status: 'finalized',
      sender: recipient.address,
      recipient: recipient.address,
      amount_atoms: (amount * K).toString(),
      nonce: String(recipient.txs - 1),
      display_name: recipient.display_name,
      block_height: String(height),
      block_hash: hash(`block-${height}`),
      timestamp: iso(minutesAgo),
    }
  }

  private makeBlock(height: number, transactions: ExplorerTransaction[], offset = 0): ExplorerBlock {
    const blockHash = hash(`block-${height}`)
    const onlineSigners = this.validatorState
      ?.filter((validator) => validator.online && validator.phase !== 'syncing' && validator.phase !== 'halted')
      .map((validator) => validator.index + 1) ?? []
    const signerIndexes = onlineSigners.length >= 3 ? onlineSigners.slice(0, 3) : [1, 2, 3]
    const signers = signerIndexes.map((index) => `validator-${index}`)
    return {
      height: String(height),
      hash: blockHash,
      parent_hash: height > 1 ? hash(`block-${height - 1}`) : '0'.repeat(64),
      proposer: `validator-${((height - 1) % 4) + 1}`,
      round: 0,
      header_proposer: `validator-${((height - 1) % 4) + 1}`,
      header_round: 0,
      timestamp: iso(offset * 1.6),
      transaction_count: transactions.length.toString(),
      transaction_root: hash(`tx-root-${height}`),
      state_root: hash(`state-${height}`),
      signers,
      certificate: {
        chain_id: 'kcoin-localnet-1',
        height: String(height),
        round: 0,
        consensus_value_hash: blockHash,
        signatures: signers.map((validator) => ({
          validator,
          signature: `${hash(`commit-a-${height}-${validator}`)}${hash(`commit-b-${height}-${validator}`)}`,
        })),
      },
      transactions,
    }
  }

  private emit(type: ApiEvent['type'], id: string): void {
    this.listeners.forEach((listener) => listener({ type, id }))
  }

  async status(): Promise<NetworkStatus> {
    const circulating = [...this.accounts.values()].reduce((sum, account) => sum + account.balance, 0n)
    const liveValidators = this.validatorState.map((validator, index) => {
      if (!validator.online || validator.phase === 'syncing') return { ...validator }
      const phaseIndex = (Math.floor(Date.now() / 1100) + index) % 4
      const phases = ['proposal', 'prevote', 'precommit', 'finalized'] as const
      return { ...validator, phase: phases[phaseIndex], height: this.height.toString() }
    })
    return {
      chain_id: 'kcoin-localnet-1',
      protocol_version: 1,
      height: this.height.toString(),
      finalized_hash: this.blocksList[0].hash,
      state_root: this.stateRoot,
      circulating_supply_atoms: circulating.toString(),
      max_supply_atoms: MAX_SUPPLY.toString(),
      mempool_size: '0',
      peer_count: String(liveValidators.filter((validator) => validator.online).length),
      block_time_ms: '1600',
      validators: liveValidators,
      syncing: false,
      halted: false,
    }
  }

  async challenge(): Promise<Challenge> {
    return { ...this.challengeValue }
  }

  async blocks(cursor?: string): Promise<Paginated<ExplorerBlock>> {
    const start = cursor ? Number(cursor) : 0
    const items = this.blocksList.slice(start, start + 10)
    return { items, next_cursor: start + 10 < this.blocksList.length ? String(start + 10) : undefined }
  }

  async block(id: string): Promise<ExplorerBlock> {
    const found = this.blocksList.find((block) => block.height === id || block.hash === id)
    if (!found) throw new Error('Block not found')
    return found
  }

  async transactions(cursor?: string): Promise<Paginated<ExplorerTransaction>> {
    const start = cursor ? Number(cursor) : 0
    return {
      items: this.transactionsList.slice(start, start + 12),
      next_cursor: start + 12 < this.transactionsList.length ? String(start + 12) : undefined,
    }
  }

  async transaction(id: string): Promise<ExplorerTransaction> {
    const found = this.transactionsList.find((transaction) => transaction.id === id)
    if (!found) throw new Error('Transaction not found')
    return found
  }

  async address(value: string): Promise<AddressSummary> {
    const account = this.accounts.get(value)
    const transactions = this.transactionsList.filter((transaction) => transaction.sender === value || transaction.recipient === value)
    return {
      address: value,
      display_name: account?.displayName,
      balance_atoms: account?.balance.toString() ?? '0',
      nonce: account?.nonce.toString() ?? '0',
      transaction_count: String(account?.transactionCount ?? transactions.length),
      first_seen_height: account?.firstSeenHeight,
      transactions,
    }
  }

  async leaderboard(): Promise<LeaderboardResponse> {
    const accounts = [...this.accounts.values()].sort((left, right) => left.balance > right.balance ? -1 : 1)
    const circulating = accounts.reduce((sum, account) => sum + account.balance, 0n)
    const share = (count: number) => Number(accounts.slice(0, count).reduce((sum, account) => sum + account.balance, 0n) * 10_000n / circulating)
    return {
      entries: accounts.slice(0, 30).map((account, index) => ({
        rank: index + 1,
        address: account.address,
        display_name: account.displayName,
        balance_atoms: account.balance.toString(),
        share_bps: Number(account.balance * 10_000n / circulating),
        transaction_count: account.transactionCount.toString(),
      })),
      circulating_supply_atoms: circulating.toString(),
      unissued_supply_atoms: (MAX_SUPPLY - circulating).toString(),
      concentration: {
        top_1_bps: share(1),
        top_5_bps: share(5),
        top_10_bps: share(10),
      },
    }
  }

  async submit(submission: TransactionSubmission): Promise<SubmissionResult> {
    const votingValidators = this.validatorState.filter((validator) =>
      validator.online && validator.phase !== 'syncing' && validator.phase !== 'halted')
    if (votingValidators.length < 3) {
      throw Object.assign(new Error('Three online validators are required to finalize.'), { code: 'FINALITY_UNAVAILABLE' })
    }
    const sender = await publicKeyToAddress(decodeBase64(submission.sender_public_key))
    const account = this.accounts.get(sender) ?? {
      address: sender,
      balance: 0n,
      nonce: 0n,
      transactionCount: 0,
      firstSeenHeight: (this.height + 1n).toString(),
    }
    if (BigInt(submission.nonce) !== account.nonce) throw Object.assign(new Error('Wallet nonce is out of date.'), { code: 'NONCE_MISMATCH' })

    let amount = 0n
    let recipient: string | undefined = sender
    if (submission.action.type === 'claim_reward') {
      const expectedAnswer = String(evaluateDemoChallenge(this.challengeValue.expression))
      if (submission.action.challenge_id !== this.challengeValue.challenge_id || submission.action.answer !== expectedAnswer) {
        throw Object.assign(new Error('That challenge answer is not valid.'), { code: 'STALE_CHALLENGE' })
      }
      const supplyBeforeClaim = [...this.accounts.values()].reduce((sum, current) => sum + current.balance, 0n)
      amount = demoRewardForSupply(supplyBeforeClaim)
      if (amount === 0n) throw Object.assign(new Error('The KCoin supply cap has been reached.'), { code: 'SUPPLY_EXHAUSTED' })
      account.balance += amount
      const supplyAfterClaim = supplyBeforeClaim + amount
      this.challengeValue = {
        challenge_id: (this.height + 1n).toString(),
        expression: '9 − 4',
        issued_at_height: (this.height + 1n).toString(),
        reward_atoms: demoRewardForSupply(supplyAfterClaim).toString(),
      }
    } else if (submission.action.type === 'transfer') {
      amount = BigInt(submission.action.amount_atoms)
      recipient = submission.action.recipient
      if (amount <= 0n || account.balance < amount) {
        throw Object.assign(new Error('This wallet does not have enough KCoin.'), { code: 'INSUFFICIENT_BALANCE' })
      }
      account.balance -= amount
      const recipientAccount = this.accounts.get(recipient) ?? {
        address: recipient,
        balance: 0n,
        nonce: 0n,
        transactionCount: 0,
        firstSeenHeight: (this.height + 1n).toString(),
      }
      recipientAccount.balance += amount
      recipientAccount.transactionCount += 1
      this.accounts.set(recipient, recipientAccount)
    } else {
      recipient = undefined
      account.displayName = submission.action.display_name ?? undefined
    }

    account.nonce += 1n
    account.transactionCount += 1
    this.accounts.set(sender, account)
    this.height += 1n
    const id = hash(`${sender}-${this.height}-${submission.nonce}`)
    const blockHash = hash(`block-${this.height}`)
    const transaction: ExplorerTransaction = {
      id,
      kind: submission.action.type,
      status: 'finalized',
      sender,
      recipient,
      amount_atoms: amount.toString(),
      nonce: submission.nonce,
      display_name: submission.action.type === 'set_display_name' ? submission.action.display_name ?? undefined : account.displayName,
      block_height: this.height.toString(),
      block_hash: blockHash,
      timestamp: new Date().toISOString(),
    }
    this.transactionsList.unshift(transaction)
    const block = this.makeBlock(Number(this.height), [transaction])
    this.blocksList.unshift(block)
    this.stateRoot = block.state_root
    this.validatorState = this.validatorState.map((validator) => validator.online ? {
      ...validator,
      height: this.height.toString(),
      block_hash: block.hash,
      state_root: block.state_root,
    } : validator)
    await new Promise((resolve) => setTimeout(resolve, 320))
    this.emit('transaction', id)
    this.emit('finalized_block', block.height)
    return { transaction_id: id, status: 'finalized' }
  }

  async setValidatorOnline(index: number, online: boolean): Promise<void> {
    const current = this.validatorState[index]
    if (!current) return
    if (!online) {
      this.validatorState[index] = { ...current, online: false, phase: 'offline', sync_progress: undefined }
      this.emit('validator_status', current.id)
      return
    }
    this.validatorState[index] = { ...current, online: true, phase: 'syncing', sync_progress: 0 }
    this.emit('validator_status', current.id)
    let progress = 0
    const timer = window.setInterval(() => {
      progress += 20
      const latest = this.validatorState[index]
      if (progress >= 100) {
        window.clearInterval(timer)
        this.validatorState[index] = {
          ...latest,
          phase: 'prevote',
          sync_progress: undefined,
          height: this.height.toString(),
          block_hash: this.blocksList[0].hash,
          state_root: this.stateRoot,
        }
      } else {
        this.validatorState[index] = { ...latest, sync_progress: progress }
      }
      this.emit('validator_status', current.id)
    }, 800)
  }

  subscribe(onEvent: (event: ApiEvent) => void): () => void {
    this.listeners.add(onEvent)
    const timer = window.setInterval(() => onEvent({ type: 'validator_status', id: 'phase' }), 1800)
    return () => {
      this.listeners.delete(onEvent)
      window.clearInterval(timer)
    }
  }
}

export function createDemoTransport(): ExplorerTransport {
  return new DemoChain()
}

export const demoTransport = new DemoChain()
