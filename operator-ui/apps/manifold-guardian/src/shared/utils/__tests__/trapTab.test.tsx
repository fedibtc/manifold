import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { trapTab } from '@/shared/utils/trapTab';

const renderDialog = () =>
  render(
    <div role="dialog" onKeyDown={trapTab}>
      <button type="button">First</button>

      <button type="button" disabled>
        Disabled
      </button>

      <button type="button">Last</button>
    </div>
  );

describe('trapTab', () => {
  it('should wrap Tab from the last control to the first', () => {
    renderDialog();
    screen.getByRole('button', { name: 'Last' }).focus();

    fireEvent.keyDown(screen.getByRole('dialog'), { key: 'Tab' });

    expect(document.activeElement).toBe(screen.getByRole('button', { name: 'First' }));
  });

  it('should wrap Shift+Tab from the first control to the last enabled one', () => {
    renderDialog();
    screen.getByRole('button', { name: 'First' }).focus();

    fireEvent.keyDown(screen.getByRole('dialog'), { key: 'Tab', shiftKey: true });

    expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Last' }));
  });

  it('should leave Tab between inner controls to the browser', () => {
    renderDialog();
    const first = screen.getByRole('button', { name: 'First' });
    first.focus();

    const handled = !fireEvent.keyDown(screen.getByRole('dialog'), { key: 'Tab' });

    expect(handled).toBe(false);
    expect(document.activeElement).toBe(first);
  });

  it('should ignore keys other than Tab', () => {
    renderDialog();
    const last = screen.getByRole('button', { name: 'Last' });
    last.focus();

    fireEvent.keyDown(screen.getByRole('dialog'), { key: 'Enter' });

    expect(document.activeElement).toBe(last);
  });
});
