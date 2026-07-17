export type DataMode = 'live' | 'demo'
export type ConsensusPhase = 'proposal' | 'prevote' | 'precommit' | 'finalized' | 'syncing' | 'offline' | 'halted'
export type TransactionKind = 'transfer' | 'claim_reward' | 'set_display_name'
export type TransactionStatus = 'pending' | 'finalized' | 'rejected'

export interface ValidatorStatus {
  id: string
  name: string
  index: number
  online: boolean
  phase: ConsensusPhase
  height: string
  round: number
  block_hash: string
  state_root: string
  sync_progress?: number
  last_seen_ms?: string
}

export interface NetworkStatus {
  chain_id: string
  protocol_version: number
  height: string
  finalized_hash: string
  state_root: string
  circulating_supply_atoms: string
  max_supply_atoms: string
  mempool_size: string
  peer_count: string
  block_time_ms: string
  validators: ValidatorStatus[]
  syncing: boolean
  halted: boolean
}

export interface Challenge {
  challenge_id: string
  expression: string
  issued_at_height: string
  reward_atoms: string
}

export interface ExplorerTransaction {
  id: string
  kind: TransactionKind
  status: TransactionStatus
  sender: string
  recipient?: string
  amount_atoms: string
  nonce: string
  display_name?: string
  block_height?: string
  block_hash?: string
  timestamp: string
  rejection_code?: string
}

export interface ExplorerBlock {
  height: string
  hash: string
  parent_hash: string
  proposer: string
  round: number
  header_proposer: string
  header_round: number
  timestamp: string
  transaction_count: string
  transaction_root: string
  state_root: string
  signers: string[]
  certificate?: CommitCertificate
  transactions?: ExplorerTransaction[]
}

export interface CommitCertificateSignature {
  validator: string
  /** Wire-encoded signature bytes. The API currently returns hexadecimal. */
  signature: string
}

export interface CommitCertificate {
  chain_id: string
  height: string
  round: number
  consensus_value_hash: string
  signatures: CommitCertificateSignature[]
}

export interface AddressSummary {
  address: string
  display_name?: string
  balance_atoms: string
  nonce: string
  transaction_count: string
  first_seen_height?: string
  transactions: ExplorerTransaction[]
}

export interface LeaderboardEntry {
  rank: number
  address: string
  display_name?: string
  balance_atoms: string
  share_bps: number
  transaction_count: string
}

export interface LeaderboardResponse {
  entries: LeaderboardEntry[]
  next_cursor?: string
  circulating_supply_atoms: string
  unissued_supply_atoms: string
  concentration: {
    top_1_bps: number
    top_5_bps: number
    top_10_bps: number
  }
}

export interface Paginated<T> {
  items: T[]
  next_cursor?: string
}

export interface TransactionSubmission {
  protocol_version: number
  chain_id: string
  sender_public_key: string
  nonce: string
  expiry_height: string
  action:
    | { type: 'transfer'; recipient: string; amount_atoms: string }
    | { type: 'claim_reward'; challenge_id: string; answer: string }
    | { type: 'set_display_name'; display_name: string | null }
  signature: string
}

export interface SubmissionResult {
  transaction_id: string
  status: TransactionStatus
}

export interface ApiErrorBody {
  code: string
  message: string
}

export interface ApiEvent {
  type: 'finalized_block' | 'transaction' | 'validator_status'
  id: string
}

export interface ExplorerTransport {
  status(signal?: AbortSignal): Promise<NetworkStatus>
  challenge(signal?: AbortSignal): Promise<Challenge>
  blocks(cursor?: string, signal?: AbortSignal): Promise<Paginated<ExplorerBlock>>
  block(id: string, signal?: AbortSignal): Promise<ExplorerBlock>
  transactions(cursor?: string, signal?: AbortSignal): Promise<Paginated<ExplorerTransaction>>
  transaction(id: string, signal?: AbortSignal): Promise<ExplorerTransaction>
  address(address: string, signal?: AbortSignal): Promise<AddressSummary>
  leaderboard(cursor?: string, signal?: AbortSignal): Promise<LeaderboardResponse>
  submit(transaction: TransactionSubmission, signal?: AbortSignal): Promise<SubmissionResult>
  setValidatorOnline?(index: number, online: boolean): Promise<void>
  subscribe(onEvent: (event: ApiEvent) => void): () => void
}

export interface WalletSession {
  address: string
  publicKey: string
  publicKeyBytes: Uint8Array
  privateKey: CryptoKey
  publicKeyCrypto: CryptoKey
  displayName: string
}
