import { useCallback, useEffect, useRef, useState } from 'react';

const FEEDBACK_RESET_MS = 1_500;

type CopyOutcome = 'idle' | 'copied' | 'failed';

// A fleet manager is commonly reached over plain http at its host address, and
// in that non-secure context `navigator.clipboard` is not defined at all. The
// async API is tried first and this selection-based path carries the value when
// it is missing or refuses, so the control is not dead on the exact deployment
// operators use.
const copyBySelection = (value: string): boolean => {
  const field = document.createElement('textarea');
  field.value = value;
  field.setAttribute('readonly', '');
  field.style.position = 'fixed';
  field.style.top = '0';
  field.style.opacity = '0';
  document.body.appendChild(field);
  field.select();

  try {
    return document.execCommand('copy');
  } catch {
    return false;
  } finally {
    field.remove();
  }
};

export const useCopyToClipboard = () => {
  const [outcome, setOutcome] = useState<CopyOutcome>('idle');
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(
    () => () => {
      if (timer.current) clearTimeout(timer.current);
    },
    []
  );

  // A failure is reported rather than swallowed: a control that does nothing and
  // says nothing reads as broken, which is worse than a stated refusal.
  const copy = useCallback(async (value: string) => {
    let succeeded = false;
    try {
      await navigator.clipboard.writeText(value);
      succeeded = true;
    } catch {
      succeeded = copyBySelection(value);
    }

    setOutcome(succeeded ? 'copied' : 'failed');
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(() => setOutcome('idle'), FEEDBACK_RESET_MS);
  }, []);

  return { copied: outcome === 'copied', failed: outcome === 'failed', copy };
};
