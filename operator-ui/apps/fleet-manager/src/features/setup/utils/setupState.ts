import { AdminApiError } from '@/shared/api/errors';

/**
 * Whether the daemon said this host has no identity yet — the one condition
 * that opens the setup wizard.
 *
 * Read from `AdminErrorKind`, never from the message. The daemon carries the
 * discriminant for exactly this consumer (`crates/fman/core/src/admin.rs`), and
 * its sentence is prose the operator reads, free to be reworded without
 * breaking first-run setup.
 */
export const isNotOnboardedError = (error: unknown): boolean =>
  error instanceof AdminApiError && error.reason === 'not_onboarded';
