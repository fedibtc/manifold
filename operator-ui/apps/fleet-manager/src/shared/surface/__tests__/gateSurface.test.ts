import { afterEach, describe, expect, it, vi } from 'vitest';
import { gateSurface } from '@/shared/surface/gateSurface';

afterEach(() => {
  gateSurface.clear('boot');
  gateSurface.clear('setup');
});

describe('gateSurface', () => {
  it('should report nothing while no gate owns a surface', () => {
    expect(gateSurface.getSnapshot()).toBeNull();
  });

  it('should report the setup surface when only the setup gate owns one', () => {
    gateSurface.set('setup', 'setup');

    expect(gateSurface.getSnapshot()).toBe('setup');
  });

  it('should prefer the boot surface over the setup surface', () => {
    gateSurface.set('setup', 'setup');
    gateSurface.set('boot', 'auth');

    expect(gateSurface.getSnapshot()).toBe('auth');
  });

  it('should fall back to the setup surface when the boot gate clears its own', () => {
    gateSurface.set('setup', 'setup');
    gateSurface.set('boot', 'auth');
    gateSurface.clear('boot');

    expect(gateSurface.getSnapshot()).toBe('setup');
  });

  it('should not let one owner clear another owner value', () => {
    gateSurface.set('setup', 'setup');
    gateSurface.clear('boot');

    expect(gateSurface.getSnapshot()).toBe('setup');
  });

  it('should notify subscribers on every change', () => {
    const listener = vi.fn();
    const unsubscribe = gateSurface.subscribe(listener);

    gateSurface.set('boot', 'boot');
    gateSurface.clear('boot');
    unsubscribe();
    gateSurface.set('boot', 'daemon-error');

    expect(listener).toHaveBeenCalledTimes(2);
  });

  it('should keep a stable snapshot while nothing changes', () => {
    gateSurface.set('boot', 'boot');

    expect(gateSurface.getSnapshot()).toBe(gateSurface.getSnapshot());
  });
});
