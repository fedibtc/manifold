import { SectionCard } from '@operator-ui/common-ui';

interface PlaceholderStepProps {
  title: string;
}

export const PlaceholderStep = ({ title }: PlaceholderStepProps) => (
  <SectionCard title={title}>Coming in the next step.</SectionCard>
);
