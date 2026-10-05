import type { SupportMessage } from '@operator-ui/types';
import { render, screen } from '@testing-library/react';
import { expect, it } from 'vitest';
import { SupportThread } from '../SupportThread';

const message = (id: string, author: SupportMessage['author'], body: string, created_at: number) =>
  ({ id: id.repeat(64), author, body, created_at }) satisfies SupportMessage;

// 2023-11-14 23:50 and 23:59 UTC, then 00:10 UTC the next day.
const LATE = 1_700_005_800;

it('should mark each UTC day once and attribute Fedi messages', () => {
  render(
    <SupportThread
      messages={[
        message('a', 'operator', 'Seat 2 is down.\nSince noon.', LATE),
        message('b', 'fedi', 'Is the host running?', LATE + 540),
        message('c', 'operator', 'Yes.', LATE + 1_200)
      ]}
    />
  );

  const items = screen.getAllByRole('listitem');
  expect(items[0]).toHaveTextContent('Tuesday 14 November 2023');
  expect(items[1]).not.toHaveTextContent('November');
  expect(items[1]).toHaveTextContent('23:59 UTC · Fedi support');
  expect(items[2]).toHaveTextContent('Wednesday 15 November 2023');
  expect(items[2]).toHaveTextContent('00:10 UTC');
  expect(items[2]).not.toHaveTextContent('Fedi support');
  expect(screen.getByText(/Seat 2 is down/).textContent).toBe('Seat 2 is down.\nSince noon.');
});

it('should invite the first message when the thread is empty', () => {
  render(<SupportThread messages={[]} />);

  expect(screen.getByText('No messages yet. Write to Fedi below.')).toBeInTheDocument();
});
