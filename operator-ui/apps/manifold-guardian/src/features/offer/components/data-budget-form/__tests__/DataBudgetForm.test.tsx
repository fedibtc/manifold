import type { AdminRequest, MonthlyDataBudget } from '@operator-ui/types';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import * as adminCallModule from '@/shared/api/adminCall';
import { DataBudgetForm } from '../DataBudgetForm';

let stored: MonthlyDataBudget = '500';
const seatMonthlyData = 65;

const mockDaemon = () =>
  vi.spyOn(adminCallModule, 'adminCall').mockImplementation(async (request: AdminRequest) => {
    if (request === 'ShowCapacity') {
      const limited = stored !== 'not_set' && stored !== 'unlimited';
      return {
        max_seats: null,
        memory_max_seats: 9,
        policy_max_seats: 12,
        monthly_data_budget: stored,
        seat_monthly_data_gb: seatMonthlyData,
        // Not what 500 / 65 makes: the form must show the daemon's answer.
        data_max_seats: limited ? (stored === '50' ? 0 : 9) : null,
        available_slots: 7
      } as never;
    }
    if (typeof request !== 'string' && 'SetDataBudget' in request) {
      stored = request.SetDataBudget.monthly_data_budget;
      return {} as never;
    }
    throw new Error('unexpected admin request');
  });

const renderForm = () => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <DataBudgetForm />
    </QueryClientProvider>
  );
};

const QUESTION = 'Does your internet plan limit how much data you can use each month?';
const noLimit = () => screen.findByLabelText('No monthly limit');
const limited = () => screen.findByLabelText('Yes, my plan has a monthly limit');
const field = () => screen.findByLabelText('Monthly data for this guardian (GB)');
const save = () => screen.getByRole('button', { name: 'Save monthly data' });

afterEach(() => {
  vi.restoreAllMocks();
  stored = '500';
});

it('should seed a stored budget and say how many seats the daemon lets it cover', async () => {
  mockDaemon();
  renderForm();

  expect(await limited()).toBeChecked();
  expect(await field()).toHaveValue('500');
  expect(screen.getByText('500 GB covers 9 seats.')).toBeInTheDocument();
  expect(await field()).toHaveAccessibleDescription(
    'How much of your plan this guardian may use, in GB per 30 days (1 GB is 1,000,000,000 bytes). We estimate one seat uses about 65 GB a month.'
  );
});

it('should warn that a budget under one seat sells nothing, and keep it', async () => {
  stored = '50';
  mockDaemon();
  renderForm();

  expect(await field()).toHaveValue('50');
  expect(
    await screen.findByText(
      '50 GB is less than one seat (about 65 GB a month). This guardian will not sell new seats.'
    )
  ).toBeInTheDocument();
  expect(screen.queryByText(/covers/)).toBeNull();
});

it('should choose nothing for an operator who has not answered', async () => {
  stored = 'not_set';
  mockDaemon();
  renderForm();

  await screen.findByRole('radiogroup', { name: QUESTION });
  expect(await noLimit()).not.toBeChecked();
  expect(await limited()).not.toBeChecked();
  expect(screen.queryByLabelText('Monthly data for this guardian (GB)')).toBeNull();
  expect(save()).toBeDisabled();
});

it('should save an edited budget as its token', async () => {
  const adminCall = mockDaemon();
  renderForm();

  fireEvent.change(await field(), { target: { value: '1,200' } });
  fireEvent.click(save());

  await waitFor(() =>
    expect(adminCall).toHaveBeenCalledWith({ SetDataBudget: { monthly_data_budget: '1200' } })
  );
});

it('should save no monthly limit and hide the amount', async () => {
  const adminCall = mockDaemon();
  renderForm();

  fireEvent.click(await noLimit());
  expect(screen.queryByLabelText('Monthly data for this guardian (GB)')).toBeNull();
  fireEvent.click(save());

  await waitFor(() =>
    expect(adminCall).toHaveBeenCalledWith({ SetDataBudget: { monthly_data_budget: 'unlimited' } })
  );
});

it('should answer a first time with a limit', async () => {
  stored = 'not_set';
  const adminCall = mockDaemon();
  renderForm();

  fireEvent.click(await limited());
  fireEvent.change(await field(), { target: { value: '179' } });
  fireEvent.click(save());

  await waitFor(() =>
    expect(adminCall).toHaveBeenCalledWith({ SetDataBudget: { monthly_data_budget: '179' } })
  );
});

it.each([
  '',
  '0'
])('should refuse a limited plan with %j GB without calling the daemon', async (value) => {
  const adminCall = mockDaemon();
  renderForm();

  fireEvent.change(await field(), { target: { value } });
  fireEvent.click(save());

  await screen.findByText('Enter a whole number of GB, such as 500.');
  expect(adminCall).not.toHaveBeenCalledWith(
    expect.objectContaining({ SetDataBudget: expect.anything() })
  );
});

it('should say it is not a hard limit', async () => {
  mockDaemon();
  renderForm();

  expect(await screen.findByRole('radiogroup', { name: QUESTION })).toHaveAccessibleDescription(
    /It is not a hard limit: seats you already run keep running/
  );
});
