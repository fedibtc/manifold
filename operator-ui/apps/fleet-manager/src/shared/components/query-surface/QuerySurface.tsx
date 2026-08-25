import { QuerySurface as SharedQuerySurface } from '@operator-ui/common-ui';
import type { ComponentProps } from 'react';
import { describeActionError } from '@/shared/utils/describeActionError';

type QuerySurfaceProps = Omit<ComponentProps<typeof SharedQuerySurface>, 'describeError'>;

/**
 * The shared four-state surface, bound to this dashboard's error vocabulary.
 * The rendering is shared with FLIP; only the sentence an operator reads on a
 * failure is app-specific, so that is the single thing bound here.
 */
export const QuerySurface = (props: QuerySurfaceProps) => (
  <SharedQuerySurface {...props} describeError={describeActionError} />
);
