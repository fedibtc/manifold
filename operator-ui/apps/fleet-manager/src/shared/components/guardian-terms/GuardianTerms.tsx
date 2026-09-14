import styles from './GuardianTerms.module.css';

const TERMS_TITLE = 'Fedi-Verified Guardian Terms of Service';
// Split so a narrow screen breaks the address after the host, not mid-word. The
// break point also splits the computed link name, so the label states it whole.
const TERMS_ORIGIN = 'https://public.qgcut.org/';
const TERMS_FILE = 'Fedi-verified_Guardian_ToS.html';
const TERMS_URL = `${TERMS_ORIGIN}${TERMS_FILE}`;
const TERMS_LINK_LABEL = `${TERMS_URL} (opens in a new tab)`;

// Renders the document, never the acceptance. The setup step asks the operator
// to accept and the Authorization page only points back at the terms, so each
// surface owns its own controls. The address stays visible so the operator can
// check the domain before opening it.
export const GuardianTerms = () => (
  <div className={styles.root}>
    <span className={styles.documentTile}>
      <svg
        viewBox="0 0 24 24"
        className={styles.documentIcon}
        aria-hidden="true"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.8"
        strokeLinecap="round"
        strokeLinejoin="round"
      >
        <path d="M14 3H6a1 1 0 0 0-1 1v16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1V8z" />

        <path d="M14 3v5h5" />

        <path d="M9 13h6M9 17h6" />
      </svg>
    </span>

    <div className={styles.body}>
      <p className={styles.title}>{TERMS_TITLE}</p>

      <p className={styles.party}>Between you and Fedi, Inc.</p>

      <a
        className={styles.link}
        href={TERMS_URL}
        target="_blank"
        rel="noopener noreferrer"
        aria-label={TERMS_LINK_LABEL}
      >
        <span className={styles.url}>
          {TERMS_ORIGIN}
          <wbr />
          {TERMS_FILE}
        </span>

        <svg
          viewBox="0 0 24 24"
          className={styles.externalIcon}
          aria-hidden="true"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
          strokeLinecap="round"
          strokeLinejoin="round"
        >
          <path d="M14 4h6v6" />

          <path d="M10 14 20 4" />

          <path d="M20 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1h5" />
        </svg>
      </a>
    </div>
  </div>
);
