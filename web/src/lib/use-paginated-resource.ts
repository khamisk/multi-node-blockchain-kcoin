import { useCallback, useEffect, useRef, useState } from 'react'
import type { Paginated } from '../types'

interface PaginatedResourceState<T> {
  items: T[]
  error?: Error
  initialized: boolean
  loading: boolean
  loadingMore: boolean
  hasMore: boolean
  loadMore: () => void
  reload: () => void
}

export function usePaginatedResource<T>(
  loader: (cursor: string | undefined, signal: AbortSignal) => Promise<Paginated<T>>,
  resetDependencies: unknown[],
  refreshKey: unknown,
  keyOf: (item: T) => string,
): PaginatedResourceState<T> {
  const [items, setItems] = useState<T[]>([])
  const [nextCursor, setNextCursor] = useState<string>()
  const [error, setError] = useState<Error>()
  const [initialized, setInitialized] = useState(false)
  const [loading, setLoading] = useState(true)
  const [loadingMore, setLoadingMore] = useState(false)
  const [attempt, setAttempt] = useState(0)
  const loaderRef = useRef(loader)
  const keyOfRef = useRef(keyOf)
  const generation = useRef(0)
  const pageController = useRef<AbortController | undefined>(undefined)
  const refreshController = useRef<AbortController | undefined>(undefined)
  const initializedRef = useRef(false)
  const loadedBeyondFirst = useRef(false)
  const previousRefreshKey = useRef(refreshKey)
  loaderRef.current = loader
  keyOfRef.current = keyOf

  const reload = useCallback(() => setAttempt((value) => value + 1), [])

  useEffect(() => {
    const controller = new AbortController()
    pageController.current?.abort()
    refreshController.current?.abort()
    const currentGeneration = ++generation.current
    initializedRef.current = false
    loadedBeyondFirst.current = false
    setItems([])
    setNextCursor(undefined)
    setError(undefined)
    setInitialized(false)
    setLoading(true)
    setLoadingMore(false)
    loaderRef.current(undefined, controller.signal)
      .then((page) => {
        if (generation.current !== currentGeneration) return
        setItems(page.items)
        setNextCursor(page.next_cursor)
        setInitialized(true)
        initializedRef.current = true
      })
      .catch((reason: unknown) => {
        if (!controller.signal.aborted && generation.current === currentGeneration) {
          setError(reason instanceof Error ? reason : new Error('Unable to load history.'))
        }
      })
      .finally(() => {
        if (!controller.signal.aborted && generation.current === currentGeneration) setLoading(false)
      })
    return () => controller.abort()
  // The caller owns reset dependencies; refs keep callbacks current.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [...resetDependencies, attempt])

  useEffect(() => {
    if (Object.is(previousRefreshKey.current, refreshKey)) return
    previousRefreshKey.current = refreshKey
    if (!initializedRef.current) return

    refreshController.current?.abort()
    const controller = new AbortController()
    refreshController.current = controller
    const currentGeneration = generation.current
    loaderRef.current(undefined, controller.signal)
      .then((page) => {
        if (controller.signal.aborted || generation.current !== currentGeneration) return
        setItems((current) => {
          const freshKeys = new Set(page.items.map((item) => keyOfRef.current(item)))
          return [...page.items, ...current.filter((item) => !freshKeys.has(keyOfRef.current(item)))]
        })
        if (!loadedBeyondFirst.current) setNextCursor(page.next_cursor)
      })
      .catch((reason: unknown) => {
        if (!controller.signal.aborted && generation.current === currentGeneration) {
          setError(reason instanceof Error ? reason : new Error('Unable to refresh history.'))
        }
      })
    return () => controller.abort()
  }, [refreshKey])

  const loadMore = useCallback(() => {
    if (!nextCursor || loadingMore) return
    pageController.current?.abort()
    const controller = new AbortController()
    pageController.current = controller
    const currentGeneration = generation.current
    setLoadingMore(true)
    setError(undefined)
    loaderRef.current(nextCursor, controller.signal)
      .then((page) => {
        if (generation.current !== currentGeneration) return
        loadedBeyondFirst.current = true
        setItems((current) => {
          const existing = new Set(current.map((item) => keyOfRef.current(item)))
          return [...current, ...page.items.filter((item) => !existing.has(keyOfRef.current(item)))]
        })
        setNextCursor(page.next_cursor)
      })
      .catch((reason: unknown) => {
        if (!controller.signal.aborted && generation.current === currentGeneration) {
          setError(reason instanceof Error ? reason : new Error('Unable to load older history.'))
        }
      })
      .finally(() => {
        if (!controller.signal.aborted && generation.current === currentGeneration) setLoadingMore(false)
      })
  }, [loadingMore, nextCursor])

  return {
    items,
    error,
    initialized,
    loading,
    loadingMore,
    hasMore: Boolean(nextCursor),
    loadMore,
    reload,
  }
}
