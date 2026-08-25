import { useCallback, useEffect, useRef, useState } from 'react';

const COPIED_RESET_MS = 1_500;

// Clipboard-API failure fallback: select the address text so the operator can
// copy it manually (US-FLIP-061).
const selectContents = (element: HTMLElement | null) => {
  if (!element || typeof window === 'undefined') return;
  const selection = window.getSelection();
  if (!selection) return;
  const range = document.createRange();
  range.selectNodeContents(element);
  selection.removeAllRanges();
  selection.addRange(range);
};

export type CopyState = 'idle' | 'copied' | 'selected';

export const useCopyAddress = () => {
  const [state, setState] = useState<CopyState>('idle');
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(
    () => () => {
      if (timer.current) clearTimeout(timer.current);
    },
    []
  );

  const copy = useCallback(async (text: string, fallbackElement: HTMLElement | null) => {
    try {
      await navigator.clipboard.writeText(text);
      setState('copied');
    } catch {
      selectContents(fallbackElement);
      setState('selected');
    }
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(() => setState('idle'), COPIED_RESET_MS);
  }, []);

  return { state, copy };
};
