import { render } from '@testing-library/react';
import { expect, it } from 'vitest';
import { FediAvatar } from '../FediAvatar';

it('should show the Fedi logo as decoration only', () => {
  const { container } = render(<FediAvatar />);

  // The bubble or title next to it names Fedi, so screen readers skip the logo.
  expect(container.querySelector('img')).toHaveAttribute('alt', '');
});
