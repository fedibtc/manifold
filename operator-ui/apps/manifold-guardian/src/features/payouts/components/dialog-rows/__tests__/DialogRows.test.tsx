import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { DialogRows } from '../DialogRows';

describe('DialogRows', () => {
  it('should pair each label with its own value and caption', () => {
    render(
      <DialogRows
        rows={[
          { label: 'Balance', value: '150,000 sats' },
          { label: 'Fees', value: '≈ 750 sats', caption: 'Lightning and federation fees' }
        ]}
      />
    );

    const terms = screen.getAllByRole('term');
    const definitions = screen.getAllByRole('definition');
    expect(terms.map((term) => term.textContent)).toEqual(['Balance', 'Fees']);
    expect(definitions[0]).toHaveTextContent(/^150,000 sats$/);
    expect(definitions[1]).toHaveTextContent('≈ 750 satsLightning and federation fees');
  });
});
