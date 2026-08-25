import { render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { describe, expect, it } from 'vitest';
import type { AttentionItem } from '@/features/overview/utils/derive';
import { AttentionRow } from '../AttentionRow';

const criticalItem: AttentionItem = {
  key: 'funds-critical',
  severity: 'critical',
  title: 'Available balance critically low',
  detail: 'Top up to keep serving requests.',
  action: { label: 'Top up', path: '/funds' }
};

const warningItem: AttentionItem = {
  key: 'advertisement-stale',
  severity: 'warning',
  title: 'Advertisement is stale',
  detail: 'Republish to stay discoverable.'
};

const renderRow = (item: AttentionItem) =>
  render(
    <MemoryRouter initialEntries={['/']}>
      <ul>
        <AttentionRow item={item} />
      </ul>
    </MemoryRouter>
  );

describe('AttentionRow', () => {
  it('should render the item title and detail', () => {
    renderRow(criticalItem);

    expect(screen.getByText('Available balance critically low')).toBeTruthy();
    expect(screen.getByText('Top up to keep serving requests.')).toBeTruthy();
  });

  it('should render the action as a link to its path', () => {
    renderRow(criticalItem);

    const action = screen.getByRole('link', { name: 'Top up' });

    expect(action.getAttribute('href')).toBe('/funds');
  });

  it('should render no action link when the item has none', () => {
    renderRow(warningItem);

    expect(screen.queryByRole('link')).toBeNull();
  });
});
