import { render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import type { AttentionItem } from '@/features/overview/utils/deriveOverview';
import { AttentionRow } from '../AttentionRow';

const item: AttentionItem = {
  key: 'fed1',
  title: 'Payment federation not receiving',
  detail: 'fed1',
  path: '/wallet'
};

const renderRow = () =>
  render(
    <MemoryRouter>
      <ul>
        <AttentionRow item={item} />
      </ul>
    </MemoryRouter>
  );

it('should render the item title and detail', () => {
  renderRow();

  screen.getByText('Payment federation not receiving');
  screen.getByText('fed1');
});

it('should link Review to the item path', () => {
  renderRow();

  const action = screen.getByRole('link', { name: 'Review' });

  expect(action.getAttribute('href')).toBe('/wallet');
});
