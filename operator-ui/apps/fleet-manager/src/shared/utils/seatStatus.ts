import type { ChipTone } from '@operator-ui/common-ui';
import type { SeatHealth, SeatPhase, SeatReport } from '@operator-ui/types';

const HEALTH_LABEL: Record<SeatHealth, string> = {
  healthy: 'Healthy',
  unavailable: 'Unavailable',
  failed: 'Failed'
};

const HEALTH_TONE: Record<SeatHealth, ChipTone> = {
  healthy: 'ok',
  unavailable: 'warn',
  failed: 'bad'
};

export const describeSeatHealth = (health: SeatHealth): { label: string; tone: ChipTone } => ({
  label: HEALTH_LABEL[health],
  tone: HEALTH_TONE[health]
});

const PHASE_LABEL: Record<SeatPhase['phase'], string> = {
  created: 'Created',
  dkg_in_progress: 'DKG in progress',
  data_loss: 'Data loss',
  running: 'Running'
};

export const describeSeatPhase = (phase: SeatPhase['phase']): string => PHASE_LABEL[phase];

export const describeSeatReport = (report: SeatReport): { label: string; tone: ChipTone } => {
  if (report.state === 'decommissioned') {
    return { label: 'Decommissioned', tone: 'neutral' };
  }
  return describeSeatHealth(report.health);
};

// `phase` and `health` are separate axes of the same report, and only one of
// them settles. Formation runs once and ends at `running`; health is what the
// chip reads and it keeps changing for the rest of the seat's life. So a
// completed formation says how *often* a seat is worth asking about, never
// whether it is worth asking at all.
export const isSeatForming = (report: SeatReport): boolean =>
  report.state === 'active' && (report.phase === 'created' || report.phase === 'dkg_in_progress');

// The one report that genuinely cannot change again: a decommissioned seat has
// no health left to report, and its row shows no health chip.
export const isSeatReportFinal = (report: SeatReport): boolean => report.state === 'decommissioned';
