import type { ConfigDraft } from '@/features/setup/services/draft';

export interface StepProps {
  draft: ConfigDraft;
  onChange: (patch: Partial<ConfigDraft>) => void;
  errors: Record<string, string>;
}
