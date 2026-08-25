import { Banner } from '@operator-ui/common-ui';
import type { SeatStatusResponse } from '@operator-ui/types';

interface SeatRecoveryNoticesProps {
  report: SeatStatusResponse['report'];
}

export const SeatRecoveryNotices = ({ report }: SeatRecoveryNoticesProps) => (
  <>
    {report.state === 'active' &&
      report.health === 'unavailable' &&
      report.phase !== 'data_loss' && (
        <Banner variant="warn">
          This seat is supervised and currently recovering — not necessarily broken. Decommissioning
          is a choice, not something you need to do right now.
        </Banner>
      )}

    {report.state === 'active' && report.phase === 'data_loss' && (
      <Banner variant="error">
        This seat has formed but its guardian data is missing. Restore its backup before expecting
        it to serve the federation.
      </Banner>
    )}

    {report.state === 'active' && report.phase === 'dkg_in_progress' && (
      <Banner variant="info">
        The setup ceremony is driven by the Federation Initiator over their own protocol — there is
        no Start/Restart control here because the admin API has none.
      </Banner>
    )}
  </>
);
