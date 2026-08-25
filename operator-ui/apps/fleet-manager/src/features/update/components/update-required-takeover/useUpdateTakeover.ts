import { useEffect, useState } from 'react';
import { useOnboarding } from '@/shared/api/hooks/use-onboarding/useOnboarding';

/** The two versions the takeover names. Both are present or the screen does not
 *  appear, which is why `latest` is a string here and `string | null` on the wire. */
export interface PublishedUpdate {
  current: string;
  latest: string;
}

export interface UpdateTakeover {
  /** Null when there is nothing to say, or when the operator has said it already. */
  update: PublishedUpdate | null;
  onDismiss: () => void;
}

/**
 * Whether to take the screen over for a newer FMan release, and how to stop.
 *
 * Reads the `Onboarding` response the dashboard already fetches, so this costs
 * no call of its own.
 *
 * Dismissal is React state and nothing else. `localStorage` or a cookie would
 * turn "for this session" into "forever", and the operator would stop hearing
 * about releases they still have not installed.
 */
export const useUpdateTakeover = (): UpdateTakeover => {
  const onboarding = useOnboarding();
  const [isDismissed, setIsDismissed] = useState(false);

  const version = onboarding.data?.fman_version;

  // `update_required` is the daemon's own SemVer comparison
  // (crates/fman/core/src/admin.rs). Never re-derive it here: string ordering is
  // not SemVer ordering, so a JavaScript compare would rank 0.10.0 below 0.9.0.
  //
  // `latest` is `string | null` on the wire and can only be non-null when
  // `update_required` is true — but the type does not say so. Requiring it here
  // is the narrowing: a screen that printed an empty version would be worse than
  // one that never appeared.
  const publishedUpdate =
    version?.update_required === true && version.latest !== null
      ? { current: version.current, latest: version.latest }
      : null;

  const isOpen = publishedUpdate !== null && !isDismissed;

  // Escape dismisses, because a surface that covers everything has to be
  // closable the way every other one is. Bound only while it is open, so it
  // never swallows Escape from a screen underneath.
  useEffect(() => {
    if (!isOpen) return;

    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setIsDismissed(true);
    };

    window.addEventListener('keydown', handleKeyDown);

    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [isOpen]);

  return {
    update: isOpen ? publishedUpdate : null,
    onDismiss: () => setIsDismissed(true)
  };
};
