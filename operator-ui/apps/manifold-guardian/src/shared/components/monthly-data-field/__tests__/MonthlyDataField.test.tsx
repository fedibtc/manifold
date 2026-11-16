import { fireEvent, render, screen } from '@testing-library/react';
import { vi } from 'vitest';
import { MonthlyDataField } from '../MonthlyDataField';

const props = {
  gb: '',
  onGbChange: () => {},
  hint: 'What it does.',
  gbHint: 'How much.'
};

it('should ask for the amount only once the plan has a limit', () => {
  const onChoiceChange = vi.fn();
  const { rerender } = render(
    <MonthlyDataField {...props} choice={null} onChoiceChange={onChoiceChange} />
  );

  expect(
    screen.getByRole('radiogroup', {
      name: 'Does your internet plan limit how much data you can use each month?'
    })
  ).toBeInTheDocument();
  expect(screen.queryByLabelText('Monthly data for this guardian (GB)')).not.toBeInTheDocument();

  fireEvent.click(screen.getByLabelText('Yes, my plan has a monthly limit'));
  expect(onChoiceChange).toHaveBeenCalledWith('limited');

  rerender(<MonthlyDataField {...props} choice="limited" onChoiceChange={onChoiceChange} />);
  expect(screen.getByLabelText('Monthly data for this guardian (GB)')).toBeInTheDocument();

  rerender(<MonthlyDataField {...props} choice="unlimited" onChoiceChange={onChoiceChange} />);
  expect(screen.queryByLabelText('Monthly data for this guardian (GB)')).not.toBeInTheDocument();
});
