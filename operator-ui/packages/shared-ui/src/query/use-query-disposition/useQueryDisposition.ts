/** The part of a react-query result this policy reads. */
export interface QueryRead {
  /** `undefined` until the daemon has answered at least once. */
  data: unknown;
  isError: boolean;
  error: unknown;
  /** Epoch ms of the last successful load; react-query reports 0 when there has
   *  never been one. */
  dataUpdatedAt?: number;
  refetch: () => void;
}

/**
 * What a screen is entitled to say, given what the daemon has and has not
 * answered. Exactly four cases, and no screen may invent a fifth.
 */
export type QueryDisposition =
  | { kind: 'loading' }
  | { kind: 'failed'; error: unknown }
  | { kind: 'stale'; updatedAtMs?: number }
  | { kind: 'content' };

export interface QueryDispositionModel {
  disposition: QueryDisposition;
  /** Forces an immediate attempt on every read behind the disposition. */
  retry: () => void;
}

/** The oldest answer on screen — a mixed-age surface is only as fresh as its
 *  stalest part. */
const oldestAnswerAt = (reads: readonly QueryRead[]): number | undefined => {
  const stamps = reads.flatMap((read) => (read.dataUpdatedAt ? [read.dataUpdatedAt] : []));
  return stamps.length === 0 ? undefined : Math.min(...stamps);
};

/**
 * React-query keeps `data` through a failed refresh, so "we hold an answer" and
 * "the last attempt failed" are independent facts and a screen must read both:
 *
 * | held answer | last attempt | disposition |
 * |-------------|--------------|-------------|
 * | no          | not failed   | `loading`   |
 * | no          | failed       | `failed`    |
 * | yes         | failed       | `stale`     |
 * | yes         | not failed   | `content`   |
 *
 * The two mistakes this exists to stop are reading `loading` as an answer — the
 * Seats page told an operator their fleet was empty because it had not been
 * told otherwise — and reading `stale` as `failed`, which deletes figures the
 * screen still holds. Stale figures under a staleness marker are strictly more
 * informative than a blank screen.
 */
export const readQueryDisposition = (reads: readonly QueryRead[]): QueryDisposition => {
  const failure = reads.find((read) => read.isError);
  const holdsAnswer = reads.every((read) => read.data !== undefined);

  if (!holdsAnswer) {
    return failure ? { kind: 'failed', error: failure.error } : { kind: 'loading' };
  }
  return failure ? { kind: 'stale', updatedAtMs: oldestAnswerAt(reads) } : { kind: 'content' };
};

/**
 * One disposition over a screen's whole read set: it is only content once every
 * read has answered, and any one failure marks the surface.
 */
export const useQueryDisposition = (reads: readonly QueryRead[]): QueryDispositionModel => {
  const disposition = readQueryDisposition(reads);
  const retry = () => {
    for (const read of reads) read.refetch();
  };

  return { disposition, retry };
};
