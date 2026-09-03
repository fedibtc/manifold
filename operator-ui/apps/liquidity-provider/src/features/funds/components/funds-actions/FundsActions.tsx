import { Banner, Button, newIdempotencyKey, TextInput } from '@operator-ui/common-ui';
import type { RequestWithdrawalRequest } from '@operator-ui/types';
import { useId, useState } from 'react';
import { TopupPanel } from '@/features/funds/components/topup-panel/TopupPanel';
import { useRequestWithdrawal } from '@/features/funds/hooks/use-request-withdrawal/useRequestWithdrawal';
import { describeActionError } from '@/features/funds/utils/describeActionError';
import styles from './FundsActions.module.css';

type ActionMode = 'idle' | 'topup' | 'withdraw';

export const FundsActions = () => {
  const [mode, setMode] = useState<ActionMode>('idle');
  const [withdrawAddress, setWithdrawAddress] = useState('');
  const [withdrawAmount, setWithdrawAmount] = useState('');
  const withdrawal = useRequestWithdrawal();
  const withdrawHintId = useId();

  const handleTopUp = () => setMode('topup');
  const handleWithdrawOpen = () => setMode('withdraw');

  const trimmedAddress = withdrawAddress.trim();
  const withdrawAmountValue = Number(withdrawAmount);
  const isWithdrawValid =
    Number.isInteger(withdrawAmountValue) && withdrawAmountValue > 0 && trimmedAddress.length > 0;

  const handleWithdrawSubmit = () => {
    if (!isWithdrawValid) {
      return;
    }
    const request: RequestWithdrawalRequest = {
      withdrawal_intent_id: newIdempotencyKey(),
      address: trimmedAddress,
      amount: withdrawAmountValue,
      fee_rate_sat_per_vbyte: null
    };
    withdrawal.mutate(request);
  };

  const withdrawalId = withdrawal.data?.operation.operation_id ?? null;
  const withdrawBlockedHintId = isWithdrawValid ? undefined : withdrawHintId;

  return (
    <div className={styles.root}>
      <div className={styles.buttons}>
        <Button onClick={handleTopUp}>Top up</Button>

        <Button variant="secondary" onClick={handleWithdrawOpen}>
          Withdraw
        </Button>
      </div>

      {mode === 'topup' && <TopupPanel />}

      {mode === 'withdraw' && (
        <div className={styles.panel}>
          <TextInput
            label="Withdrawal address"
            value={withdrawAddress}
            onChange={setWithdrawAddress}
          />

          <TextInput label="Amount (sats)" value={withdrawAmount} onChange={setWithdrawAmount} />

          {/* Why the button is disabled, tied to the button itself — this is one
              control with a hint, not a labelled group, so FormField is the
              wrong shape for it. */}
          {!isWithdrawValid && (
            <p id={withdrawHintId} className={styles.submitHint}>
              Enter an address and a whole number of sats greater than 0.
            </p>
          )}

          <Button
            onClick={handleWithdrawSubmit}
            loading={withdrawal.isPending}
            disabled={!isWithdrawValid}
            describedBy={withdrawBlockedHintId}
          >
            Request withdrawal
          </Button>

          {withdrawal.isError && (
            <Banner variant="error" title="Withdrawal request failed">
              {describeActionError(withdrawal.error)}
            </Banner>
          )}

          {withdrawalId && (
            <span className={styles.panelLabel}>Requested operation {withdrawalId}</span>
          )}
        </div>
      )}
    </div>
  );
};
