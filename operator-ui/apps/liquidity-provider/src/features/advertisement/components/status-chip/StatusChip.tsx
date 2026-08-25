import { Chip, type ChipTone } from '@operator-ui/common-ui';
import type { ReactNode } from 'react';

export type { ChipTone };

interface StatusChipProps {
  tone: ChipTone;
  children: ReactNode;
}

export const StatusChip = ({ tone, children }: StatusChipProps) => (
  <Chip tone={tone}>{children}</Chip>
);
