import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { BootLoading } from '../BootLoading';

describe('BootLoading', () => {
  it('should say the app is loading rather than render an empty screen', () => {
    render(<BootLoading />);

    expect(screen.getByText('Loading…')).toBeTruthy();
  });
});
