import type { SupportMessage } from '@operator-ui/types';
import { render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { SupportThread } from '../SupportThread';

const message = (id: string, author: SupportMessage['author'], body: string, created_at: number) =>
  ({ id: id.repeat(64), author, body, created_at, unread: false }) satisfies SupportMessage;

// Local time. "Now" is 2023-11-15 12:00, so the messages fall on the day
// before yesterday, yesterday, and today.
const at = (day: number, hours: number, minutes: number) =>
  new Date(2023, 10, day, hours, minutes).getTime() / 1000;

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['Date'] });
  vi.setSystemTime(new Date(2023, 10, 15, 12, 0));
});

afterEach(() => {
  vi.useRealTimers();
  delete (Element.prototype as Partial<Element>).scrollIntoView;
});

it('should mark each new local day and time each message in its bubble', () => {
  render(
    <SupportThread
      messages={[
        message('a', 'operator', 'Seat 2 is down.\nSince noon.', at(13, 23, 59)),
        message('b', 'fedi', 'Is the host running?', at(14, 0, 1)),
        message('c', 'operator', 'Yes.', at(14, 12, 5)),
        message('d', 'fedi', 'Restart it, please.', at(15, 9, 30))
      ]}
    />
  );

  const items = screen.getAllByRole('listitem');
  expect(items.map((item) => item.querySelector(':scope > p')?.textContent ?? null)).toEqual([
    'Monday, 13 November',
    'Yesterday',
    null,
    'Today'
  ]);
  expect(items.map((item) => item.querySelector('time')?.textContent)).toEqual([
    '23:59',
    '00:01',
    '12:05',
    '09:30'
  ]);
  expect(items[1]).toHaveTextContent('Guardian support: Is the host running?');
  expect(items[2]).toHaveTextContent('You: Yes.');
  expect(screen.getByText(/Seat 2 is down/).textContent).toBe('You: Seat 2 is down.\nSince noon.');
});

it('should show the first unread message rather than the newest', () => {
  // jsdom does not implement scrolling.
  const scrolled: Element[] = [];
  Element.prototype.scrollIntoView = function (this: Element) {
    scrolled.push(this);
  };

  // The unread Fedi message comes before a read one, so the newest message
  // is not the one to show.
  render(
    <SupportThread
      messages={[
        message('a', 'operator', 'Seat 2 is down.', at(14, 9, 0)),
        { ...message('b', 'fedi', 'Is the host running?', at(14, 9, 5)), unread: true },
        message('c', 'fedi', 'Any news?', at(15, 9, 0)),
        { ...message('d', 'fedi', 'Restart it, please.', at(15, 9, 30)), unread: true }
      ]}
    />
  );

  expect(scrolled.at(-1)).toBe(screen.getAllByRole('listitem')[1]);
});

it('should invite the first message when the thread is empty', () => {
  render(<SupportThread messages={[]} />);

  expect(screen.getByText('No messages yet. Write to guardian support below.')).toBeInTheDocument();
});
