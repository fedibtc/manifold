import { CopyButton } from '@operator-ui/common-ui';

interface CopyAuthorizationRequestProps {
  request: string;
}

// The request is the JSON the holder app parses, not the bare key, so it
// gets a control that says which of the two it carries. An unlabelled icon
// beside the key read as "copy this key" and handed over something else.
export const CopyAuthorizationRequest = ({ request }: CopyAuthorizationRequestProps) => (
  <CopyButton value={request} label="Copy the authorization request" showLabel />
);
