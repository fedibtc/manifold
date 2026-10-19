import { type ReactNode, useState } from 'react';
import styles from './PeerBadgeHelp.module.css';

const FEDI_SUPPORT_URL = 'https://support.fedi.xyz';

type HelpTopic = 'what' | 'badge' | 'trouble';

interface TopicCard {
  id: HelpTopic;
  title: string;
  icon: ReactNode;
}

interface Issue {
  title: string;
  detail: ReactNode;
}

const TOPICS: TopicCard[] = [
  {
    id: 'what',
    title: 'What is PeerBadge?',
    icon: (
      <svg className={styles.cardIcon} viewBox="0 0 24 24" aria-hidden="true">
        <path d="M12 6.5C10 5 7 4.5 4 5v13c3-.5 6 0 8 1.5 2-1.5 5-2 8-1.5V5c-3-.5-6 0-8 1.5Z" />

        <path d="M12 6.5V19" />
      </svg>
    )
  },
  {
    id: 'badge',
    title: 'Need a Trusted badge?',
    icon: (
      <svg className={styles.cardIcon} viewBox="0 0 24 24" aria-hidden="true">
        <circle cx="9" cy="8" r="3.2" />

        <path d="M3.5 19c.6-3.2 2.8-5 5.5-5s4.9 1.8 5.5 5" />

        <circle cx="17" cy="9" r="2.4" />

        <path d="M16 14.2c2.3.2 4 1.8 4.5 4.8" />
      </svg>
    )
  },
  {
    id: 'trouble',
    title: 'Having trouble?',
    icon: (
      <svg className={styles.cardIcon} viewBox="0 0 24 24" aria-hidden="true">
        <circle cx="12" cy="12" r="9" />

        <path d="M9.6 9.3a2.5 2.5 0 0 1 4.8.9c0 1.7-2.4 2.2-2.4 3.6" />

        <path d="M12 17h.01" />
      </svg>
    )
  }
];

const ISSUES: Issue[] = [
  {
    title: 'Your camera opens text or a web search',
    detail: 'Scan from Authorize application inside PeerBadge, not your phone’s camera.'
  },
  {
    title: 'This page is open on your phone',
    detail: 'Copy the authorization request, then tap Paste code in PeerBadge.'
  },
  {
    title: 'No Authorize application button',
    detail: 'New badges stay locked for a while. Try again later.'
  },
  {
    title: 'Not sure it’s the right host',
    detail: 'The app key in PeerBadge matches the Manifold Fedimint Guardian ID above.'
  },
  {
    title: 'Still waiting after approving',
    detail: 'Wait a minute, then click Check status again.'
  }
];

const HELP_PANEL_ID = 'peerbadge-help-panel';

const renderIssue = (issue: Issue) => (
  <li key={issue.title} className={styles.item}>
    <p className={styles.title}>{issue.title}</p>

    <p className={styles.text}>{issue.detail}</p>
  </li>
);

const PANELS: Record<HelpTopic, ReactNode> = {
  what: (
    <p className={styles.text}>
      PeerBadge is a verification protocol that Fedi uses to anonymously attest that the guardians
      that Federations select are human, unique, and trustworthy. Your badge shows Fedi has checked
      who you are, and those who initiate Manifold Federations (Federation Ambassadors) only see
      hosts approved with a Trusted badge.
    </p>
  ),
  badge: (
    <p className={styles.text}>
      Fedi gives you one in a 10-minute call. Ask your Fedi contact or{' '}
      <a className={styles.link} href={FEDI_SUPPORT_URL} target="_blank" rel="noreferrer">
        Fedi Support
      </a>
      . If your badge says “Basic,” ask them to upgrade it to “Trusted.”
    </p>
  ),
  trouble: <ul className={styles.issues}>{ISSUES.map(renderIssue)}</ul>
};

export const PeerBadgeHelp = () => {
  const [openTopic, setOpenTopic] = useState<HelpTopic | null>(null);

  const renderCard = (topic: TopicCard) => {
    const isOpen = openTopic === topic.id;
    const handleClick = () => {
      setOpenTopic((current) => (current === topic.id ? null : topic.id));
    };

    return (
      <button
        key={topic.id}
        type="button"
        className={styles.card}
        aria-expanded={isOpen}
        aria-controls={isOpen ? HELP_PANEL_ID : undefined}
        onClick={handleClick}
      >
        {topic.icon}

        <span className={styles.cardTitle}>{topic.title}</span>
      </button>
    );
  };

  return (
    <div className={styles.root}>
      <div className={styles.cards}>{TOPICS.map(renderCard)}</div>

      {openTopic ? (
        <div id={HELP_PANEL_ID} className={styles.panel}>
          {PANELS[openTopic]}
        </div>
      ) : null}
    </div>
  );
};
