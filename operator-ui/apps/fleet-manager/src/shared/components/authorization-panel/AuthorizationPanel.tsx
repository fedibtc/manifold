import { Banner, CopyButton } from '@operator-ui/common-ui';
import type { OnboardingResponse } from '@operator-ui/types';
import { QRCodeSVG } from 'qrcode.react';
import { AuthorizationStatusBanner } from '@/shared/components/authorization-status-banner/AuthorizationStatusBanner';
import { buildAuthorizationRequest } from '@/shared/utils/authorization';
import { describeActionError } from '@/shared/utils/describeActionError';
import styles from './AuthorizationPanel.module.css';

const QR_SIZE = 192;

interface AuthorizationPanelProps {
  data: OnboardingResponse | undefined;
  isLoading: boolean;
  error: unknown;
}

// Renders state, never actions. The setup step and the standalone page need the
// same key, QR and status but entirely different controls, so the buttons stay
// with each surface.
export const AuthorizationPanel = ({ data, isLoading, error }: AuthorizationPanelProps) => {
  if (!data) {
    if (error) return <Banner variant="error">{describeActionError(error)}</Banner>;
    if (isLoading) return <p className={styles.loading}>Reading the authorization state…</p>;
    return null;
  }

  const request = buildAuthorizationRequest(data);

  return (
    <div className={styles.root}>
      <div className={styles.qrBox}>
        <QRCodeSVG value={request} size={QR_SIZE} marginSize={2} />
      </div>

      {/* The key is shown whole, unlike BackupPage, which truncates the same class
          of value. That page lists keys for reference; this one shows the single
          value a holder may compare against their own application, and a truncated
          value does not permit that comparison. */}
      <div className={styles.keyRow}>
        <code className={styles.key}>{data.service_nostr_pubkey}</code>

        <CopyButton value={data.service_nostr_pubkey} label="Copy the service Nostr public key" />
      </div>

      {/* The request is the JSON the holder app parses, not the bare key, so it
          gets a control that says which of the two it carries. An unlabelled icon
          beside the key read as "copy this key" and handed over something else. */}
      <div className={styles.requestRow}>
        <CopyButton value={request} label="Copy the authorization request" showLabel />
      </div>

      {/* The holder side is credential-app PR #132, which accepts exactly this
          payload: `parseHolderAuthorizationRequest` takes
          `{ subject_pubkey: <64 hex> }` through a `.strict()` schema, and the
          same request serves FMan and FLIP. Strict is why
          `buildAuthorizationRequest` may never gain a field — an added `type` or
          `version` would be rejected outright, so BE-FMAN-AUTH-002 has to be
          agreed on both sides at once. */}
      <p className={styles.hint}>
        This is the fleet manager's service Nostr public key, as the daemon reports it. A holder
        signs an authorization over this key. Scan the code with the holder application, or copy the
        request and paste it in.
      </p>

      <AuthorizationStatusBanner nostr={data.nostr} />
      {error ? (
        <Banner variant="warn">
          This state could not be refreshed: {describeActionError(error)}
        </Banner>
      ) : null}
    </div>
  );
};
