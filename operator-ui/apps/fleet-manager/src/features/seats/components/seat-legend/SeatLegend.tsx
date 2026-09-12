import { SectionCard } from '@operator-ui/common-ui';
import type { SeatHealth, SeatPhase } from '@operator-ui/types';
import { describeSeatHealth, describeSeatPhase } from '@/shared/utils/seatStatus';
import styles from './SeatLegend.module.css';

interface LegendEntry {
  term: string;
  description: string;
}

// Labels come from the shared describers rather than being retyped here, so a
// renamed status cannot leave the legend explaining a word the table no longer
// shows.
const HEALTH_ORDER: SeatHealth[] = ['healthy', 'unavailable', 'failed'];

const HEALTH_MEANING: Record<SeatHealth, string> = {
  healthy: 'serving normally',
  unavailable: 'temporarily not serving; expected to recover on its own',
  failed: 'the fleet gave up restarting it — this one needs you'
};

const PHASE_ORDER: SeatPhase['phase'][] = ['created', 'dkg_in_progress', 'running', 'data_loss'];

const PHASE_MEANING: Record<SeatPhase['phase'], string> = {
  created: 'paid for, not yet set up',
  dkg_in_progress: 'the federation is generating its keys',
  running: 'set up and live',
  data_loss: 'its data is gone and it needs recovery'
};

const join = (parts: string[]) => parts.join(' · ');

const ENTRIES: LegendEntry[] = [
  {
    term: 'Seat',
    description: "This seat's ID. Open it for the invite code and its guardian fee account."
  },
  {
    term: 'FI',
    description:
      'The Federation Initiator who bought the seat, shown as their public key. It is here so you can match a seat to a buyer — nothing on this dashboard acts on it.'
  },
  {
    term: 'Plan',
    description: 'What the buyer paid, at the price the seat sold for.'
  },
  {
    term: 'Phase',
    description: join(
      PHASE_ORDER.map((phase) => `${describeSeatPhase(phase)} — ${PHASE_MEANING[phase]}`)
    )
  },
  {
    term: 'Health',
    description: join(
      HEALTH_ORDER.map(
        (health) => `${describeSeatHealth(health).label} — ${HEALTH_MEANING[health]}`
      )
    )
  }
];

const renderEntry = (entry: LegendEntry) => (
  <div key={entry.term} className={styles.entry}>
    <dt className={styles.term}>{entry.term}</dt>

    <dd className={styles.description}>{entry.description}</dd>
  </div>
);

/**
 * What each column means. The table's own headers are single words the daemon
 * chose, and three of them (FI, Phase, Health) name a value set an operator has
 * no other way to enumerate — a seat only ever shows the one state it is in.
 */
export const SeatLegend = () => (
  <SectionCard title="What these columns mean">
    <dl className={styles.root}>{ENTRIES.map(renderEntry)}</dl>
  </SectionCard>
);
