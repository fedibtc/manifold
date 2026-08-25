import { Banner } from '../banner/Banner';

interface StaleDataBannerProps {
  /** Epoch ms of the last successful load. Omitted when the query has never
   *  recorded one, in which case the banner drops the timestamp clause. */
  updatedAtMs?: number;
}

/**
 * Shown above content that is still on screen after a poll failed. The screen
 * keeps the last-good data rather than blanking, so this banner is what stops
 * an operator from reading stale figures as current ones.
 */
export const StaleDataBanner = ({ updatedAtMs }: StaleDataBannerProps) => {
  const detail = updatedAtMs
    ? `Retrying the connection — last updated ${new Date(updatedAtMs).toLocaleTimeString()}.`
    : 'Retrying the connection.';

  return (
    <Banner variant="warn" title="Showing last-known data">
      {detail}
    </Banner>
  );
};
