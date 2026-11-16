import { Banner } from '@operator-ui/common-ui';
import type { CapacityResponse } from '@operator-ui/types';
import { Link } from 'react-router-dom';
import {
  budgetGb,
  describeBelowOneSeat,
  describeSeatEstimate,
  formatGb,
  seatCount
} from '@/shared/utils/dataBudget';
import styles from './DataUseSummary.module.css';

interface DataUseSummaryProps {
  capacity: CapacityResponse;
  activeSeats: number;
}

const SETTINGS_PATH = '/offer';

/** The monthly data budget against the per-seat estimate: what the budget
 *  allows, and whether it is what stops sales now. */
export const DataUseSummary = ({ capacity, activeSeats }: DataUseSummaryProps) => {
  const {
    monthly_data_budget: answer,
    seat_monthly_data_gb: perSeat,
    data_max_seats: covered
  } = capacity;
  const budget = budgetGb(answer);

  // An MG set up before the question: nothing is wrong, so no warning.
  if (answer === 'not_set') {
    return (
      <p>
        <Link to={SETTINGS_PATH} className={styles.link}>
          Answer the monthly data question
        </Link>
      </p>
    );
  }

  // Only a GB budget gives a seat count.
  if (budget === null || covered === null) {
    return (
      <div className={styles.root}>
        <p>
          Your internet plan has no monthly data limit, so data use does not limit how many seats
          this host sells.
        </p>

        <p className={styles.note}>{describeSeatEstimate(perSeat)}</p>

        <Link to={SETTINGS_PATH} className={styles.link}>
          Change monthly data
        </Link>
      </div>
    );
  }

  // A budget under one seat covers none, whether or not any seat runs: say
  // that, not "full".
  const belowOneSeat = covered === 0;
  const full = !belowOneSeat && activeSeats >= covered;

  return (
    <div className={styles.root}>
      {belowOneSeat ? (
        <Banner variant="warn">{describeBelowOneSeat(budget, perSeat)}</Banner>
      ) : null}

      {full ? (
        <Banner variant="info" title="Full for your monthly data">
          The seats this host runs already use what your monthly data covers, so it is not selling
          new ones. Running seats keep running. To sell more, raise your monthly data.
        </Banner>
      ) : null}

      <dl className={styles.facts}>
        <div className={styles.fact}>
          <dt>Your monthly data</dt>

          <dd>{formatGb(budget)}</dd>
        </div>

        <div className={styles.fact}>
          <dt>Estimated per seat</dt>

          <dd>About {formatGb(perSeat)} a month</dd>
        </div>

        <div className={styles.fact}>
          <dt>Seats it covers</dt>

          <dd>{seatCount(covered)}</dd>
        </div>

        <div className={styles.fact}>
          <dt>Seats running</dt>

          <dd>{seatCount(activeSeats)}</dd>
        </div>
      </dl>

      <p className={styles.note}>
        This is an estimate, not a meter: busy federations can use more.
      </p>

      <Link to={SETTINGS_PATH} className={styles.link}>
        Change monthly data
      </Link>
    </div>
  );
};
