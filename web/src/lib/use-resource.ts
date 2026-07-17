import { useCallback, useEffect, useRef, useState } from 'react'

interface ResourceState<T> {
  data?: T
  error?: Error
  loading: boolean
  reload: () => void
}

export function useResource<T>(loader: (signal: AbortSignal) => Promise<T>, dependencies: unknown[]): ResourceState<T> {
  const [data, setData] = useState<T>()
  const [error, setError] = useState<Error>()
  const [loading, setLoading] = useState(true)
  const [attempt, setAttempt] = useState(0)
  const loaderRef = useRef(loader)
  loaderRef.current = loader
  const reload = useCallback(() => setAttempt((value) => value + 1), [])

  useEffect(() => {
    const controller = new AbortController()
    setLoading(true)
    setError(undefined)
    loaderRef.current(controller.signal)
      .then((value) => setData(value))
      .catch((reason: unknown) => {
        if (!controller.signal.aborted) setError(reason instanceof Error ? reason : new Error('Unable to load data.'))
      })
      .finally(() => {
        if (!controller.signal.aborted) setLoading(false)
      })
    return () => controller.abort()
  // Dependencies intentionally belong to the caller; loader is kept current through a ref.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [...dependencies, attempt])

  return { data, error, loading, reload }
}
