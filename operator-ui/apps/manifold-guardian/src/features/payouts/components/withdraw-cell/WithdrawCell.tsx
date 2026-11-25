import { Button } from '@operator-ui/common-ui';
import styles from './WithdrawCell.module.css';

interface WithdrawCellProps {
  /** The balance is known to hold nothing. An unread balance is not empty:
   *  the daemon is the authority on whether there is anything to take. */
  isEmpty: boolean;
  /** Which balance, for screen readers: every row's button says Withdraw. */
  balanceName: string;
  onWithdraw: () => void;
}

/** One balance row's only action. */
export const WithdrawCell = ({ isEmpty, balanceName, onWithdraw }: WithdrawCellProps) =>
  isEmpty ? (
    <span className={styles.empty}>Nothing to withdraw</span>
  ) : (
    <Button size="small" variant="secondary" onClick={onWithdraw}>
      Withdraw<span className={styles.hidden}> {balanceName}</span>
    </Button>
  );
