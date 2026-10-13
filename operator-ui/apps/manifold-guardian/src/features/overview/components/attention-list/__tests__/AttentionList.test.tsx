import { render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import type { AttentionItem } from '@/features/overview/utils/deriveOverview';
import { AttentionList } from '../AttentionList';

const items: AttentionItem[] = [
  { key: 'fed1', title: 'Payment federation not receiving', detail: 'fed1', path: '/wallet' },
  { key: 'fed2', title: 'Payment federation not receiving', detail: 'fed2', path: '/wallet' }
];

const renderList = (attention: AttentionItem[]) =>
  render(
    <MemoryRouter>
      <AttentionList items={attention} />
    </MemoryRouter>
  );

it('should render a row per attention item under the section title', () => {
  renderList(items);

  screen.getByText('Needs attention');
  screen.getByText('fed1');
  screen.getByText('fed2');
});

it('should render nothing when there are no attention items', () => {
  const { container } = renderList([]);

  expect(container.innerHTML).toBe('');
});
