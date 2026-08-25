import { renderHook } from '@testing-library/react';
import { vi } from 'vitest';
import { type QueryRead, readQueryDisposition, useQueryDisposition } from '../useQueryDisposition';

const read = (overrides: Partial<QueryRead> = {}): QueryRead => ({
  data: undefined,
  isError: false,
  error: null,
  refetch: () => {},
  ...overrides
});

it('should report loading while nothing has answered and nothing has failed', () => {
  expect(readQueryDisposition([read()])).toEqual({ kind: 'loading' });
});

it('should report failed when nothing has answered and a read failed', () => {
  const error = new Error('daemon down');

  expect(readQueryDisposition([read({ isError: true, error })])).toEqual({ kind: 'failed', error });
});

it('should report stale when an answer is held and a read failed', () => {
  const reads = [read({ data: { seats: [] }, isError: true, error: new Error('blip') })];

  expect(readQueryDisposition(reads)).toEqual({ kind: 'stale', updatedAtMs: undefined });
});

it('should report content when every read has answered and none failed', () => {
  expect(readQueryDisposition([read({ data: { seats: [] } })])).toEqual({ kind: 'content' });
});

it('should stay loading until every read has answered', () => {
  const reads = [read({ data: { seats: [] } }), read()];

  expect(readQueryDisposition(reads)).toEqual({ kind: 'loading' });
});

it('should mark the whole surface stale when one of several reads failed', () => {
  const reads = [
    read({ data: { seats: [] }, dataUpdatedAt: 2_000 }),
    read({ data: { plans: [] }, dataUpdatedAt: 1_000, isError: true, error: new Error('blip') })
  ];

  expect(readQueryDisposition(reads)).toEqual({ kind: 'stale', updatedAtMs: 1_000 });
});

it('should date a stale surface by its oldest answer', () => {
  const reads = [
    read({ data: {}, dataUpdatedAt: 5_000, isError: true, error: new Error('blip') }),
    read({ data: {}, dataUpdatedAt: 3_000 })
  ];

  expect(readQueryDisposition(reads)).toEqual({ kind: 'stale', updatedAtMs: 3_000 });
});

it('should prefer failed over stale when one read has no answer at all', () => {
  const error = new Error('never answered');
  const reads = [read({ data: { seats: [] } }), read({ isError: true, error })];

  expect(readQueryDisposition(reads)).toEqual({ kind: 'failed', error });
});

it('should retry every read behind the disposition', () => {
  const first = vi.fn();
  const second = vi.fn();
  const { result } = renderHook(() =>
    useQueryDisposition([read({ refetch: first }), read({ refetch: second })])
  );

  result.current.retry();

  expect(first).toHaveBeenCalledTimes(1);
  expect(second).toHaveBeenCalledTimes(1);
});
