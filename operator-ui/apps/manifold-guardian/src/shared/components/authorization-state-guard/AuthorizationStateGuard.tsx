import { Banner } from '@operator-ui/common-ui';
import type { OnboardingResponse } from '@operator-ui/types';
import type { ReactNode } from 'react';
import { describeActionError } from '@/shared/utils/describeActionError';
import styles from './AuthorizationStateGuard.module.css';

interface AuthorizationStateGuardProps {
  data: OnboardingResponse | undefined;
  isLoading: boolean;
  error: unknown;
  children: (data: OnboardingResponse) => ReactNode;
}

export const AuthorizationStateGuard = ({
  data,
  isLoading,
  error,
  children
}: AuthorizationStateGuardProps) => {
  if (!data) {
    if (error) return <Banner variant="error">{describeActionError(error)}</Banner>;
    if (isLoading) return <p className={styles.loading}>Reading the authorization state…</p>;
    return null;
  }

  return (
    <div className={styles.root}>
      {children(data)}
      {error ? (
        <Banner variant="warn">
          This state could not be refreshed: {describeActionError(error)}
        </Banner>
      ) : null}
    </div>
  );
};
