import type { OnboardingResponse } from '@operator-ui/types';
import { AuthorizationQrCode } from '@/shared/components/authorization-qr-code/AuthorizationQrCode';
import { CopyAuthorizationRequest } from '@/shared/components/copy-authorization-request/CopyAuthorizationRequest';
import { ServiceKeyRow } from '@/shared/components/service-key-row/ServiceKeyRow';
import { buildAuthorizationRequest } from '@/shared/utils/authorization';
import styles from './PanelAuthorizationCode.module.css';

const QR_SIZE = 192;

interface PanelAuthorizationCodeProps {
  data: OnboardingResponse;
}

export const PanelAuthorizationCode = ({ data }: PanelAuthorizationCodeProps) => {
  const request = buildAuthorizationRequest(data);

  return (
    <>
      <AuthorizationQrCode request={request} size={QR_SIZE} />

      <ServiceKeyRow pubkey={data.service_nostr_pubkey} />

      <div className={styles.requestRow}>
        <CopyAuthorizationRequest request={request} />
      </div>
    </>
  );
};
