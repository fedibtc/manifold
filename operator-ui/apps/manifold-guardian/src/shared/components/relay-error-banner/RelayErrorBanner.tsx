import { Banner } from '@operator-ui/common-ui';
import type { ReactNode } from 'react';

interface RelayErrorBannerProps {
  error: string;
  action?: ReactNode;
}

export const RelayErrorBanner = ({ error, action }: RelayErrorBannerProps) => (
  <Banner variant="warn" title="Approval could not be checked" action={action}>
    This host may or may not be approved — this is a connection problem, not a refusal: {error}
  </Banner>
);
