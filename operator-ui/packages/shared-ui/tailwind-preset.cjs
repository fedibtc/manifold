// Shared Tailwind preset consumed by both apps (FLIP + FMan).
// Tokens mirror the Fedi Design System — source of truth is
// fedi/ui/common/constants/theme.ts (HEX_COLORS + theme.colors). Visual
// reference only; no shared runtime code. Token names track the Fedi names so
// drift diffs cleanly. When the Fedi tokens change, re-sync from theme.ts (or
// the fedi-design MCP `get_design_tokens`) and update the fallbacks here.
// Apps set their own `content`; this preset only carries theme tokens + plugins.

const { join } = require('node:path');
const { utilitiesPlugin } = require('./load-utilities.cjs');

/** @type {import('tailwindcss').Config} */
module.exports = {
  theme: {
    extend: {
      colors: {
        // Primary text = Fedi `night`; page surface = white.
        // ink + muted are alpha-aware (space-separated RGB triplet + <alpha-value>)
        // so opacity modifiers work — e.g. text-ink/50, border-muted/50.
        // night #0B1013 = 11 16 19; dividerGrey #EAEAEB = 234 234 235.
        ink: 'rgb(var(--color-night-rgb, 11 16 19) / <alpha-value>)',
        surface: 'var(--color-white, #FFFFFF)',
        muted: 'rgb(var(--color-divider-grey-rgb, 234 234 235) / <alpha-value>)',
        gray: {
          50: 'var(--color-grey-50, #F8F8F8)',
          150: 'var(--color-divider-grey, #EAEAEB)',
          200: 'var(--color-light-grey, #D3D4DB)',
          400: 'var(--color-grey-400, #9C9EA0)',
          500: 'var(--color-dark-grey, #6D7071)'
        },
        strong: 'var(--color-light-grey, #D3D4DB)',
        error: 'var(--color-red, #E00A00)',
        success: 'var(--color-green, #00A829)',
        warning: 'var(--color-orange, #DF7B00)',
        info: 'var(--color-blue, #0277F2)',
        infoSoft: 'var(--color-blue-100, #D6F2FF)',
        warnSoft: 'var(--color-orange-100, #FFF5C5)',
        errorSoft: 'var(--color-red-100, #FFDFDD)',
        successSoft: 'var(--color-green-100, #D7FFE0)',
        status: {
          healthy: 'var(--color-green, #00A829)',
          warning: 'var(--color-orange, #DF7B00)',
          failed: 'var(--color-red, #E00A00)'
        }
      },
      fontFamily: {
        sans: ['Poppins', 'ui-sans-serif', 'system-ui', 'sans-serif']
      },
      borderRadius: {
        card: '12px'
      },
      // Content column and sidebar rail, from the wireframe shell: a 220px
      // sidebar beside a 960px content column (904px inside its padding).
      maxWidth: {
        content: '60rem'
      },
      gridTemplateColumns: {
        shell: '13.75rem 1fr'
      }
    }
  },
  // Cross-app `u`-utilities are registered from the shared dictionary here, so
  // `@apply uPageHeading` resolves in every module (dev + build). See
  // load-utilities.cjs for why the CSS file can't do this on its own.
  plugins: [utilitiesPlugin(join(__dirname, 'styles/utilities.css'))]
};
