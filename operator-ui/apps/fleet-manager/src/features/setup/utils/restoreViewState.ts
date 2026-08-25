import type { AdminErrorKind } from '@operator-ui/types';
import { AdminApiError, AuthError } from '@/shared/api/errors';
import { describeActionError } from '@/shared/utils/describeActionError';

/** The parts of `OnboardFromBackupResponse` the screen shows. Deliberately not the
 *  response itself: nothing that ever held the phrase enters view state. */
export interface SafeRestoreResult {
  seats: number;
  formed: number;
}

/**
 * `daemon`  — the daemon refused before it installed the identity.
 * `auth`    — the authentication middleware refused before dispatch.
 * `network` — the result is unknown; the daemon may have installed the identity.
 */
export type RestoreErrorClass = 'daemon' | 'auth' | 'network';

export interface SafeRestoreError {
  errorClass: RestoreErrorClass;
  message: string;
  /**
   * Why the daemon refused, on the `daemon` class only — the other two classes
   * are the browser's own reading of a failure, and inventing a discriminant
   * for them would be a guess.
   *
   * This is what lets the screen pick an action. It is carried but not yet
   * branched on: which action each refusal offers is copy, and copy on this
   * screen is @miki's.
   */
  reason?: AdminErrorKind;
}

const UNKNOWN_RESULT_MESSAGE = 'The connection to the fleet manager failed before it answered.';

export type RestoreViewState =
  | { type: 'form' }
  | { type: 'success'; result: SafeRestoreResult }
  | { type: 'failed'; error: SafeRestoreError }
  | { type: 'unknown'; error: SafeRestoreError };

/**
 * The daemon's restore errors already name the cause and the action, so a daemon
 * refusal is passed through word for word — and now with the daemon's own
 * discriminant beside it, so a screen can select an action without matching
 * that prose. That is what `BE-FMAN-RECOVERY-003` asked for.
 *
 * Anything that is neither a daemon refusal nor an authentication refusal is read
 * as an unknown result, including a bare thrown value: the browser cannot tell a
 * lost response from a request that never arrived.
 *
 * The unknown class carries a fixed message rather than `describeActionError`'s.
 * That describer ends every transport failure with an invitation to try again,
 * which is sound for a read but is the one instruction this class must never
 * give: a second restore cannot be undone.
 */
export const classifyRestoreError = (error: unknown): SafeRestoreError => {
  if (error instanceof AdminApiError)
    return { errorClass: 'daemon', message: error.message, reason: error.reason };
  if (error instanceof AuthError)
    return { errorClass: 'auth', message: describeActionError(error) };
  return { errorClass: 'network', message: UNKNOWN_RESULT_MESSAGE };
};
