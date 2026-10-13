import type { SeatStatusResponse, SeatSummary } from '@operator-ui/types';
import { useSeatReports } from '@/features/seats/api/hooks/use-seat-reports/useSeatReports';
import { useSeats } from '@/shared/api/hooks/use-seats/useSeats';
import {
  type QueryDisposition,
  useQueryDisposition
} from '@/shared/query/use-query-disposition/useQueryDisposition';

export interface SeatRow {
  seat: SeatSummary;
  report?: SeatStatusResponse['report'];
  reportLoading: boolean;
}

export interface SeatRowsModel {
  rows: SeatRow[];
  activeCount: number;
  decommissionedCount: number;
  /** True only when the daemon answered and listed no seats. An unanswered or
   *  failed read is not an empty fleet, and saying so is a claim about the
   *  operator's inventory that nothing supports. */
  isEmpty: boolean;
  /** Covers the seat list only. A per-seat status that fails is already carried
   *  by the row it belongs to, and must not blank the list around it. */
  disposition: QueryDisposition;
  retry: () => void;
}

export const useSeatRows = (): SeatRowsModel => {
  const seats = useSeats();
  const { disposition, retry } = useQueryDisposition([seats]);
  const allSeats = seats.data?.seats ?? [];
  const activeSeats = allSeats.filter((seat) => !seat.decommissioned);
  const decommissionedSeats = allSeats.filter((seat) => seat.decommissioned);
  const reports = useSeatReports(activeSeats.map((seat) => seat.seat_id));
  const reportBySeatId = new Map(activeSeats.map((seat, index) => [seat.seat_id, reports[index]]));

  const rows: SeatRow[] = allSeats.map((seat) => {
    const report = reportBySeatId.get(seat.seat_id);
    return { seat, report: report?.data?.report, reportLoading: report?.isPending ?? false };
  });

  return {
    rows,
    activeCount: activeSeats.length,
    decommissionedCount: decommissionedSeats.length,
    isEmpty: seats.data !== undefined && allSeats.length === 0,
    disposition,
    retry
  };
};
