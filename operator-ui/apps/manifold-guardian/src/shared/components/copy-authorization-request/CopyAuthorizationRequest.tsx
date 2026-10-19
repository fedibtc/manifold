import { CopyButton } from '@operator-ui/common-ui';

interface CopyAuthorizationRequestProps {
  request: string;
}

export const CopyAuthorizationRequest = ({ request }: CopyAuthorizationRequestProps) => (
  <CopyButton value={request} label="Copy the authorization request" showLabel />
);
