import type { OnboardingResponse } from '@operator-ui/types';
import homeShot from '@/shared/assets/peerbadge/step-1-home.webp';
import badgeDetailsShot from '@/shared/assets/peerbadge/step-2-authorize-application.webp';
import { AuthorizeApplicationCallout } from '@/shared/components/authorize-application-callout/AuthorizeApplicationCallout';
import { MyBadgesCallout } from '@/shared/components/my-badges-callout/MyBadgesCallout';
import { PeerBadgePhoneMockup } from '@/shared/components/peerbadge-phone-mockup/PeerBadgePhoneMockup';
import { PeerBadgeStepHead } from '@/shared/components/peerbadge-step-head/PeerBadgeStepHead';
import { StepAuthorizationCode } from '@/shared/components/step-authorization-code/StepAuthorizationCode';
import { PEERBADGE_APP_HOST, PEERBADGE_APP_URL } from '@/shared/utils/peerbadge';
import styles from './PeerBadgeApprovalSteps.module.css';

interface PeerBadgeApprovalStepsProps {
  data: OnboardingResponse;
}

export const PeerBadgeApprovalSteps = ({ data }: PeerBadgeApprovalStepsProps) => {
  return (
    <div className={styles.steps}>
      <section className={styles.column}>
        <PeerBadgeStepHead number={1} title="Open PeerBadge">
          Go to{' '}
          <a className={styles.link} href={PEERBADGE_APP_URL} target="_blank" rel="noreferrer">
            {PEERBADGE_APP_HOST}
          </a>{' '}
          and tap My badges.
        </PeerBadgeStepHead>

        <PeerBadgePhoneMockup
          src={homeShot}
          alt="PeerBadge on your phone: What do you want to do? with My badges at the top"
        >
          <MyBadgesCallout />
        </PeerBadgePhoneMockup>
      </section>

      <section className={styles.column}>
        <PeerBadgeStepHead number={2} title="Open your badge">
          Tap your badge, then Authorize application. PeerBadge opens a scanner.
        </PeerBadgeStepHead>

        <PeerBadgePhoneMockup
          src={badgeDetailsShot}
          alt="PeerBadge on your phone: your Trusted badge with Authorize application below it"
        >
          <AuthorizeApplicationCallout />
        </PeerBadgePhoneMockup>
      </section>

      <section className={styles.column}>
        <PeerBadgeStepHead number={3} title="Scan this code">
          Then tap Authorize and publish.
        </PeerBadgeStepHead>

        <StepAuthorizationCode data={data} />
      </section>
    </div>
  );
};
