import { fireEvent, render, screen } from '@testing-library/react';
import { vi } from 'vitest';
import { CheckboxField } from '../CheckboxField';

it('should toggle and call onChange with the boolean', () => {
  const onChange = vi.fn();
  render(<CheckboxField label="Advertise readiness" checked={false} onChange={onChange} />);

  fireEvent.click(screen.getByLabelText('Advertise readiness'));
  expect(onChange).toHaveBeenCalledWith(true);
});
