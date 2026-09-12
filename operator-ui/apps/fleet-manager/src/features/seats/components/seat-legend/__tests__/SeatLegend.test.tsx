import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { SeatLegend } from '../SeatLegend';

describe('SeatLegend', () => {
  it('should explain what the FI column is for', () => {
    render(<SeatLegend />);

    expect(screen.getByText(/Federation Initiator who bought the seat/i)).toBeInTheDocument();
  });

  it('should enumerate every health a seat can report', () => {
    render(<SeatLegend />);

    const health = screen.getByText(/serving normally/i).textContent ?? '';

    expect(health).toContain('Healthy');
    expect(health).toContain('Unavailable');
    expect(health).toContain('Failed');
  });

  it('should enumerate every formation phase a seat can report', () => {
    render(<SeatLegend />);

    const phase = screen.getByText(/generating its keys/i).textContent ?? '';

    expect(phase).toContain('Created');
    expect(phase).toContain('DKG in progress');
    expect(phase).toContain('Running');
    expect(phase).toContain('Data loss');
  });
});
