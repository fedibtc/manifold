import { Banner, Button, Chip } from '@operator-ui/common-ui';
import type { AttestationPayloadInfo } from '@operator-ui/types';
import { type ChangeEvent, useRef, useState } from 'react';
import { useAttestationList } from '@/features/attestations/api/hooks/use-attestation-list/useAttestationList';
import { useInstallAttestation } from '@/features/attestations/hooks/use-install-attestation/useInstallAttestation';
import { useRemoveAttestation } from '@/features/attestations/hooks/use-remove-attestation/useRemoveAttestation';
import {
  ATTESTATION_KIND_LABELS,
  formatSubject
} from '@/features/attestations/utils/formatAttestation';
import { describeActionError } from '@/shared/utils/describeActionError';
import { formatDate } from '@/shared/utils/format';
import styles from './AttestationPanel.module.css';

export const AttestationPanel = () => {
  const list = useAttestationList();
  const install = useInstallAttestation();
  const remove = useRemoveAttestation();
  const fileInputRef = useRef<HTMLInputElement>(null);
  const [selectedFile, setSelectedFile] = useState<File | null>(null);

  const handleFileChange = (event: ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0] ?? null;
    setSelectedFile(file);
  };

  const handleInstall = () => {
    if (!selectedFile) return;
    install.mutate(selectedFile, {
      onSuccess: () => {
        setSelectedFile(null);
        if (fileInputRef.current) fileInputRef.current.value = '';
      }
    });
  };

  const handleRemove = (payload: AttestationPayloadInfo) => {
    remove.mutate({ target: { id: payload.id } });
  };

  const removingId =
    remove.isPending && remove.variables && 'id' in remove.variables.target
      ? remove.variables.target.id
      : null;

  return (
    <div className={styles.root}>
      <div className={styles.install}>
        <input
          ref={fileInputRef}
          type="file"
          aria-label="Attestation file"
          className={styles.fileInput}
          onChange={handleFileChange}
        />

        <Button
          variant="primary"
          size="small"
          onClick={handleInstall}
          loading={install.isPending}
          disabled={!selectedFile}
        >
          Install
        </Button>
      </div>

      {install.isError ? (
        <Banner variant="error" title="Couldn't install attestation">
          {describeActionError(install.error)}
        </Banner>
      ) : null}

      {remove.isError ? (
        <Banner variant="error" title="Couldn't remove attestation">
          {describeActionError(remove.error)}
        </Banner>
      ) : null}

      {list.isLoading ? <p className={styles.muted}>Loading attestations…</p> : null}

      {list.isError ? (
        <Banner variant="error" title="Couldn't load attestations">
          {describeActionError(list.error)}
        </Banner>
      ) : null}

      {list.isSuccess && list.data.payloads.length === 0 ? (
        <p className={styles.muted}>No attestations installed yet.</p>
      ) : null}

      {list.isSuccess && list.data.payloads.length > 0 ? (
        <ul className={styles.list}>
          {list.data.payloads.map((payload) => {
            const handleRowRemove = () => handleRemove(payload);
            return (
              <li key={payload.id} className={styles.row}>
                <div className={styles.meta}>
                  <span className={styles.kind}>{ATTESTATION_KIND_LABELS[payload.kind]}</span>

                  <span className={styles.subject}>{formatSubject(payload.subject)}</span>

                  <span className={styles.date}>{formatDate(payload.ingested_at)}</span>

                  <Chip tone={payload.valid ? 'ok' : 'bad'}>
                    {payload.valid ? 'Valid' : 'Invalid'}
                  </Chip>
                </div>

                <Button
                  variant="secondary"
                  size="small"
                  onClick={handleRowRemove}
                  loading={removingId === payload.id}
                >
                  Remove
                </Button>
              </li>
            );
          })}
        </ul>
      ) : null}
    </div>
  );
};
