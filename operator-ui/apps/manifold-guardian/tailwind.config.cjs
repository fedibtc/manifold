const { join } = require('node:path');
const preset = require('@operator-ui/common-ui/tailwind-preset');
const { utilitiesPlugin } = require('@operator-ui/common-ui/load-utilities');

/** @type {import('tailwindcss').Config} */
module.exports = {
  presets: [preset],
  content: ['./index.html', './src/**/*.{ts,tsx}', '../../packages/shared-ui/src/**/*.{ts,tsx}'],
  // App-local `u`-utilities loaded from the app's shared dictionary so
  // `@apply uMono` resolves in modules (dev + build). See load-utilities.cjs.
  plugins: [utilitiesPlugin(join(__dirname, 'src/shared/styles/utilities.css'))]
};
