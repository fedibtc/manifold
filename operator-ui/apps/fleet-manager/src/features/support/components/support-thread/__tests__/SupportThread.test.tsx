import type { SupportMessage } from '@operator-ui/types';
import { render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { SupportThread } from '../SupportThread';

const message = (id: string, author: SupportMessage['author'], body: string, created_at: number) =>
  ({ id: id.repeat(64), author, body, created_at }) satisfies SupportMessage;

// 2023-11-14 23:50:00 UTC. "Now" is the next UTC day, so only the last group
// below falls on today.
const T = 1_700_005_800;

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['Date'] });
  vi.setSystemTime(new Date('2023-11-15T12:00:00Z'));
});

afterEach(() => {
  vi.useRealTimers();
});

it('should time each group of messages, as the Fedi app chat does', () => {
  render(
    <SupportThread
      messages={[
        message('a', 'operator', 'Seat 2 is down.\nSince noon.', T),
        // 60 s after the one before: same group, no new time.
        message('b', 'operator', 'Any idea?', T + 60),
        message('c', 'fedi', 'Is the host running?', T + 120),
        // 61 s after the one before: a new group.
        message('d', 'operator', 'Yes.', T + 181),
        message('e', 'fedi', 'Restart it, please.', T + 1_200)
      ]}
    />
  );

  const items = screen.getAllByRole('listitem');
  expect(items.map((item) => item.querySelector('time')?.textContent ?? null)).toEqual([
    'Nov 14, 23:50 UTC',
    null,
    null,
    'Nov 14, 23:53 UTC',
    '00:10 UTC'
  ]);
  expect(items.map((item) => item.dataset.spacing ?? null)).toEqual([
    null,
    'run',
    'turn',
    null,
    null
  ]);
  expect(items[2]).toHaveTextContent('Fedi support: Is the host running?');
  expect(items[3]).toHaveTextContent('You: Yes.');
  expect(screen.getByText(/Seat 2 is down/).textContent).toBe('You: Seat 2 is down.\nSince noon.');
});

it('should invite the first message when the thread is empty', () => {
  render(<SupportThread messages={[]} />);

  expect(screen.getByText('No messages yet. Write to Fedi below.')).toBeInTheDocument();
});
