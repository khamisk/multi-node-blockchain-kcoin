import { act, renderHook, waitFor } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { usePaginatedResource } from '../lib/use-paginated-resource'

interface Item {
  id: string
}

describe('paginated explorer history', () => {
  it('keeps loaded older pages when a finalized-history refresh arrives', async () => {
    let firstPageLoads = 0
    const loader = async (cursor: string | undefined) => {
      if (cursor === 'older') return { items: [{ id: 'c' }, { id: 'd' }], next_cursor: undefined }
      firstPageLoads += 1
      return firstPageLoads === 1
        ? { items: [{ id: 'a' }, { id: 'b' }], next_cursor: 'older' }
        : { items: [{ id: 'new' }, { id: 'a' }], next_cursor: 'older' }
    }

    const { result, rerender } = renderHook(
      ({ revision }) => usePaginatedResource<Item>(loader, [], revision, (item) => item.id),
      { initialProps: { revision: 0 } },
    )
    await waitFor(() => expect(result.current.items.map((item) => item.id)).toEqual(['a', 'b']))

    act(() => result.current.loadMore())
    await waitFor(() => expect(result.current.items.map((item) => item.id)).toEqual(['a', 'b', 'c', 'd']))

    rerender({ revision: 1 })
    await waitFor(() => expect(result.current.items.map((item) => item.id)).toEqual(['new', 'a', 'b', 'c', 'd']))
  })
})
