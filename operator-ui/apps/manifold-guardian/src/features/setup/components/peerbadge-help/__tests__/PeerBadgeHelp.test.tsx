import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { PeerBadgeHelp } from '../PeerBadgeHelp';

const openTopic = (name: RegExp) => fireEvent.click(screen.getByRole('button', { name }));

describe('PeerBadgeHelp', () => {
  it('should start folded, with three topics side by side', () => {
    render(<PeerBadgeHelp />);

    const cards = screen.getAllByRole('button');
    expect(cards).toHaveLength(3);
    expect(cards.every((card) => card.getAttribute('aria-expanded') === 'false')).toBe(true);
    expect(screen.queryByText(/a verification protocol that Fedi uses/)).toBeNull();
  });

  it('should explain what PeerBadge is and why it is needed', () => {
    render(<PeerBadgeHelp />);

    openTopic(/What is PeerBadge\?/);

    expect(screen.getByText(/only see hosts approved with a Trusted badge/)).toBeTruthy();
  });

  it('should explain how to get a Trusted badge', () => {
    render(<PeerBadgeHelp />);

    openTopic(/Need a Trusted badge\?/);

    expect(screen.getByRole('link', { name: 'Fedi Support' })).toBeTruthy();
  });

  it('should list fixes without repeating the steps', () => {
    render(<PeerBadgeHelp />);

    openTopic(/Having trouble\?/);

    expect(screen.queryAllByRole('img')).toHaveLength(0);
    expect(screen.getByText('No Authorize application button')).toBeTruthy();
    expect(screen.getByText(/click Check status again/)).toBeTruthy();
  });

  it('should fold a topic when it is clicked again', () => {
    render(<PeerBadgeHelp />);

    openTopic(/What is PeerBadge\?/);
    openTopic(/What is PeerBadge\?/);

    expect(screen.queryByText(/a verification protocol that Fedi uses/)).toBeNull();
  });
});
