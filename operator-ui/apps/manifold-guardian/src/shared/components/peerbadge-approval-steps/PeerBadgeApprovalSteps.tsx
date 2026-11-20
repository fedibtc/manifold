import type { OnboardingResponse } from '@operator-ui/types';
import homeShot from '@/shared/assets/peerbadge/step-1-home.webp';
import badgeDetailsShot from '@/shared/assets/peerbadge/step-2-authorize-application.webp';
import { AuthorizeApplicationCallout } from '@/shared/components/authorize-application-callout/AuthorizeApplicationCallout';
import { HolderPairing } from '@/shared/components/holder-pairing/HolderPairing';
import { MyBadgesCallout } from '@/shared/components/my-badges-callout/MyBadgesCallout';
import { PeerBadgePhoneMockup } from '@/shared/components/peerbadge-phone-mockup/PeerBadgePhoneMockup';
import { PeerBadgeStepHead } from '@/shared/components/peerbadge-step-head/PeerBadgeStepHead';
import { StepAuthorizationCode } from '@/shared/components/step-authorization-code/StepAuthorizationCode';
import { PEERBADGE_APP_HOST, PEERBADGE_APP_URL } from '@/shared/utils/peerbadge';
import styles from './PeerBadgeApprovalSteps.module.css';

interface PeerBadgeApprovalStepsProps {
  data: OnboardingResponse;
  privatePairing?: boolean;
}

export const PeerBadgeApprovalSteps = ({
  data,
  privatePairing = false
}: PeerBadgeApprovalStepsProps) => {
  return (
    <div className={styles.guide}>
      <div className={styles.steps}>
        <section className={styles.column}>
          <PeerBadgeStepHead number={1} title="Open PeerBadge">
            Go to{' '}
            <a className={styles.link} href={PEERBADGE_APP_URL} target="_blank" rel="noreferrer">
              {PEERBADGE_APP_HOST}
            </a>{' '}
            {privatePairing ? 'in an up-to-date browser and tap My badges.' : 'and tap My badges.'}
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
            {privatePairing ? 'Tap your Trusted badge' : 'Tap your badge'}, then Authorize
            application. PeerBadge opens a scanner.
          </PeerBadgeStepHead>

          <PeerBadgePhoneMockup
            src={badgeDetailsShot}
            alt="PeerBadge on your phone: your Trusted badge with Authorize application below it"
          >
            <AuthorizeApplicationCallout />
          </PeerBadgePhoneMockup>
        </section>

        <section className={styles.column}>
          <PeerBadgeStepHead
            number={3}
            title={privatePairing ? 'Pair with this host' : 'Scan this code'}
          >
            {privatePairing
              ? 'Create and scan the private code. Verify the app key matches the Guardian ID, then tap Authorize and pair.'
              : 'Then tap Authorize and publish.'}
          </PeerBadgeStepHead>

          {privatePairing ? <HolderPairing /> : <StepAuthorizationCode data={data} />}
        </section>
      </div>
    </div>
  );
};
