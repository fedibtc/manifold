import type { CapacityResponse } from '@operator-ui/types';
import { render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { expect, it } from 'vitest';
import { DataUseSummary } from '../DataUseSummary';

const capacity = (overrides: Partial<CapacityResponse> = {}): CapacityResponse => ({
  max_seats: null,
  memory_max_seats: 9,
  policy_max_seats: 12,
  cpu_max_seats: null,
  cpu_benchmark: null,
  disk_max_seats: null,
  monthly_data_budget: '500',
  seat_monthly_data_gb: 65,
  data_max_seats: 7,
  available_slots: 5,
  ...overrides
});

const renderSummary = (response: CapacityResponse, activeSeats: number) =>
  render(
    <MemoryRouter>
      <DataUseSummary capacity={response} activeSeats={activeSeats} />
    </MemoryRouter>
  );

const fact = (term: string) => screen.getByText(term).nextElementSibling?.textContent;

it('should show the budget, the estimate, and the seats it covers', () => {
  renderSummary(capacity(), 3);

  expect(fact('Your monthly data')).toBe('500 GB');
  expect(fact('Estimated per seat')).toBe('About 65 GB a month');
  expect(fact('Seats it covers')).toBe('7 seats');
  expect(fact('Seats running')).toBe('3 seats');
  expect(screen.queryByText('Full for your monthly data')).toBeNull();
});

// One seat short of the limit still sells; at the limit it stops.
it('should not warn one seat below what the budget covers', () => {
  renderSummary(capacity(), 6);

  expect(screen.queryByText('Full for your monthly data')).toBeNull();
});

it('should say the host is full for its data once the seats use the budget', () => {
  renderSummary(capacity(), 7);

  expect(screen.getByText('Full for your monthly data')).toBeInTheDocument();
  expect(screen.getByText(/Running seats keep running/)).toBeInTheDocument();
  // This card does not read the readiness checks, so it claims nothing about them.
  expect(screen.queryByText(/readiness/i)).toBeNull();
});

// 50 / 65 covers no seat. With no seat running, "full" would be false.
it.each([
  0, 2
])('should say a budget under one seat sells nothing, with %i running, not full', (running) => {
  renderSummary(capacity({ monthly_data_budget: '50', data_max_seats: 0 }), running);

  expect(
    screen.getByText(
      '50 GB is less than one seat (about 65 GB a month). This guardian will not sell new seats.'
    )
  ).toBeInTheDocument();
  expect(screen.queryByText('Full for your monthly data')).toBeNull();
  expect(fact('Seats it covers')).toBe('0 seats');
});

// 65 GB is exactly one seat: covered, so no under-one-seat warning.
it('should not warn at exactly one seat', () => {
  renderSummary(capacity({ monthly_data_budget: '65', data_max_seats: 1 }), 0);

  expect(screen.queryByText(/less than one seat/)).toBeNull();
  expect(screen.queryByText('Full for your monthly data')).toBeNull();
});

it('should say data does not limit seats for a plan without a cap', () => {
  renderSummary(capacity({ monthly_data_budget: 'unlimited', data_max_seats: null }), 3);

  expect(
    screen.getByText(
      'Your internet plan has no monthly data limit, so data use does not limit how many seats this host sells.'
    )
  ).toBeInTheDocument();
  expect(screen.getByText('We estimate one seat uses about 65 GB a month.')).toBeInTheDocument();
  expect(screen.getByRole('link', { name: 'Change monthly data' })).toHaveAttribute(
    'href',
    '/offer'
  );
});

// An MG set up before the question: one neutral line, no warning, no figures.
it('should ask an operator who has not answered, without a warning', () => {
  renderSummary(capacity({ monthly_data_budget: 'not_set', data_max_seats: null }), 3);

  expect(screen.getByRole('link', { name: 'Answer the monthly data question' })).toHaveAttribute(
    'href',
    '/offer'
  );
  expect(screen.queryByRole('alert')).toBeNull();
  expect(screen.queryByText('Your monthly data')).toBeNull();
  expect(screen.queryByText(/no monthly data limit/)).toBeNull();
});
