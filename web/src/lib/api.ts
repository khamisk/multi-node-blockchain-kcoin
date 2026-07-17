import type {
  AddressSummary,
  ApiErrorBody,
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
} from '../types'

export class ApiError extends Error {
  constructor(public readonly code: string, message: string, public readonly status?: number) {
    super(message)
    this.name = 'ApiError'
  }
}

function normalizeBase(value: string | undefined): string {
  return (value ?? '').replace(/\/$/, '')
}

export class RestTransport implements ExplorerTransport {
  constructor(private readonly baseUrl = normalizeBase(import.meta.env.VITE_API_BASE_URL)) {}

  private async request<T>(path: string, init?: RequestInit): Promise<T> {
    const response = await fetch(`${this.baseUrl}${path}`, {
      ...init,
      headers: { Accept: 'application/json', ...init?.headers },
    })
    if (!response.ok) {
      let error: ApiErrorBody = { code: 'NETWORK_ERROR', message: `Request failed (${response.status}).` }
      try { error = await response.json() as ApiErrorBody } catch { /* retain stable fallback */ }
      throw new ApiError(error.code, error.message, response.status)
    }
    return response.json() as Promise<T>
  }

  status(signal?: AbortSignal): Promise<NetworkStatus> {
    return this.request('/api/v1/status', { signal })
  }

  challenge(signal?: AbortSignal): Promise<Challenge> {
    return this.request('/api/v1/challenge', { signal })
  }

  blocks(cursor?: string, signal?: AbortSignal): Promise<Paginated<ExplorerBlock>> {
    const query = cursor ? `?cursor=${encodeURIComponent(cursor)}` : ''
    return this.request(`/api/v1/blocks${query}`, { signal })
  }

  block(id: string, signal?: AbortSignal): Promise<ExplorerBlock> {
    return this.request(`/api/v1/blocks/${encodeURIComponent(id)}`, { signal })
  }

  transactions(cursor?: string, signal?: AbortSignal): Promise<Paginated<ExplorerTransaction>> {
    const query = cursor ? `?cursor=${encodeURIComponent(cursor)}` : ''
    return this.request(`/api/v1/transactions${query}`, { signal })
  }

  transaction(id: string, signal?: AbortSignal): Promise<ExplorerTransaction> {
    return this.request(`/api/v1/transactions/${encodeURIComponent(id)}`, { signal })
  }

  address(address: string, signal?: AbortSignal): Promise<AddressSummary> {
    return this.request(`/api/v1/addresses/${encodeURIComponent(address)}`, { signal })
  }

  leaderboard(cursor?: string, signal?: AbortSignal): Promise<LeaderboardResponse> {
    const query = cursor ? `?cursor=${encodeURIComponent(cursor)}` : ''
    return this.request(`/api/v1/leaderboard${query}`, { signal })
  }

  submit(transaction: TransactionSubmission, signal?: AbortSignal): Promise<SubmissionResult> {
    return this.request('/api/v1/transactions', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(transaction),
      signal,
    })
  }

  subscribe(onEvent: (event: ApiEvent) => void): () => void {
    const source = new EventSource(`${this.baseUrl}/api/v1/events`)
    const consume = (event: MessageEvent<string>) => {
      try { onEvent(JSON.parse(event.data) as ApiEvent) } catch { /* ignore malformed stream event */ }
    }
    source.onmessage = consume
    ;['finalized_block', 'transaction', 'validator_status'].forEach((type) => source.addEventListener(type, consume as EventListener))
    return () => source.close()
  }
}
