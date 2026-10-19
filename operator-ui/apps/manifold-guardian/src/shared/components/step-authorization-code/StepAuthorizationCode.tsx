import type { OnboardingResponse } from '@operator-ui/types';
import { AuthorizationQrCode } from '@/shared/components/authorization-qr-code/AuthorizationQrCode';
import { CopyAuthorizationRequest } from '@/shared/components/copy-authorization-request/CopyAuthorizationRequest';
import { ServiceKeyRow } from '@/shared/components/service-key-row/ServiceKeyRow';
import { buildAuthorizationRequest } from '@/shared/utils/authorization';
import styles from './StepAuthorizationCode.module.css';

const QR_SIZE = 128;

interface StepAuthorizationCodeProps {
  data: OnboardingResponse;
}

export const StepAuthorizationCode = ({ data }: StepAuthorizationCodeProps) => {
  const request = buildAuthorizationRequest(data);

  return (
    <div className={styles.code}>
      <AuthorizationQrCode request={request} size={QR_SIZE} />

      <p className={styles.or}>or</p>

      <div className={styles.copyPill}>
        <CopyAuthorizationRequest request={request} />
      </div>

      <div className={styles.idBlock}>
        <span className={styles.idLabel}>Manifold Fedimint Guardian ID</span>

        <ServiceKeyRow pubkey={data.service_nostr_pubkey} />
      </div>
    </div>
  );
};
