import react from '@vitejs/plugin-react';
import { defineConfig } from 'vitest/config';

export default defineConfig({
  // The React Compiler memoizes render output, and both apps build with it (see
  // each app's vite.config.ts). Without it here the panel tests would exercise a
  // component that re-renders far more eagerly than the shipped one, and would
  // pass over staleness bugs the browser shows immediately.
  plugins: [react({ babel: { plugins: ['babel-plugin-react-compiler'] } })],
  test: {
    globals: true,
    environment: 'jsdom',
    setupFiles: ['./src/test-setup.ts'],
    coverage: {
      provider: 'v8',
      reporter: ['text-summary', 'json-summary', 'lcov'],
      include: ['src/**']
    }
  }
});
