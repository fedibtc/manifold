import { Banner, Button, CopyButton, SectionCard } from '@operator-ui/common-ui';
import type { SeatGrantEntry } from '@operator-ui/types';
import { AuthorizationQrCode } from '@/shared/components/authorization-qr-code/AuthorizationQrCode';
import { formatCheckedAt } from '@/shared/utils/format';
import styles from './GuardianCodeShare.module.css';

/** A code the MG still shares: only an unused one carries its URI. */
export type ShareableGrant = SeatGrantEntry & { status: { state: 'unused'; uri: string } };

export const isShareable = (grant: SeatGrantEntry): grant is ShareableGrant =>
  grant.status.state === 'unused';

interface GuardianCodeShareProps {
  grant: ShareableGrant;
  isNew: boolean;
  onDone: () => void;
}

export const GuardianCodeShare = ({ grant, isNew, onDone }: GuardianCodeShareProps) => (
  <SectionCard title="Share with your Federation Ambassador">
    <div className={styles.root}>
      {isNew ? (
        <Banner variant="success" title="Guardian code ready">
          Send it privately, for example in Fedi Chat, or show the QR code in person.
        </Banner>
      ) : null}

      <div className={styles.body}>
        <AuthorizationQrCode request={grant.status.uri} size={200} />

        <div className={styles.details}>
          <dl className={styles.facts}>
            <div className={styles.fact}>
              <dt className={styles.term}>Label</dt>

              <dd className={styles.value}>{grant.label ?? 'No label'}</dd>
            </div>

            <div className={styles.fact}>
              <dt className={styles.term}>Use</dt>

              <dd className={styles.value}>One federation setup</dd>
            </div>

            <div className={styles.fact}>
              <dt className={styles.term}>Setup fee</dt>

              <dd className={styles.value}>Free</dd>
            </div>

            <div className={styles.fact}>
              <dt className={styles.term}>Created</dt>

              <dd className={styles.value}>{formatCheckedAt(grant.issued_at)}</dd>
            </div>
          </dl>

          <div className={styles.code}>
            <span className={styles.codeText}>{grant.status.uri}</span>
          </div>

          <CopyButton value={grant.status.uri} label="Copy guardian code" showLabel />
        </div>
      </div>

      <p className={styles.note}>
        Viewing or copying this code does not use it. It is used once, when a Federation Ambassador
        sets up a federation with it. Until then you can revoke it from the list.
      </p>

      <div className={styles.actions}>
        <Button onClick={onDone}>Done</Button>
      </div>
    </div>
  </SectionCard>
);
