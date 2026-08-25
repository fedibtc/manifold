import { render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { describe, expect, it } from 'vitest';
import type { AttentionItem } from '@/features/overview/utils/derive';
import { AttentionList } from '../AttentionList';

const items: AttentionItem[] = [
  {
    key: 'funds-critical',
    severity: 'critical',
    title: 'Available balance critically low',
    detail: 'Top up to keep serving requests.',
    action: { label: 'Top up', path: '/funds' }
  },
  {
    key: 'advertisement-failed',
    severity: 'warning',
    title: 'Advertisement failed to publish',
    detail: 'No relay accepted the listing.'
  }
];

const renderList = (attention: AttentionItem[]) =>
  render(
    <MemoryRouter initialEntries={['/']}>
      <AttentionList items={attention} />
    </MemoryRouter>
  );

describe('AttentionList', () => {
  it('should render a row per attention item under the section title', () => {
    renderList(items);

    expect(screen.getByText('Needs attention')).toBeTruthy();
    expect(screen.getByText('Available balance critically low')).toBeTruthy();
    expect(screen.getByText('Advertisement failed to publish')).toBeTruthy();
  });

  it('should render nothing when there are no attention items', () => {
    const { container } = renderList([]);

    expect(container.innerHTML).toBe('');
  });
});
