import { Buffer } from 'buffer';
import { dataToFrames } from 'qrloop';

/**
 * Bytes for qrloop's `dataToFrames`, encoded as the Fedi app encodes them so its
 * scanner reassembles them (fedi `ui/common/utils/qrFrames.ts`): a mint-v2 token
 * ("fedimint"-prefixed base32) as its UTF-8 text, a mint-v1 token (base64) as
 * the bytes it decodes to. Decoding a v2 token as base64 would corrupt it, since
 * every character of it is valid base64.
 */
export const ecashToQrFrameData = (ecash: string): Buffer =>
  ecash.startsWith('fedimint') ? Buffer.from(ecash, 'utf8') : Buffer.from(ecash, 'base64');

/** The animated QR frames that carry `ecash`, in display order. */
export const ecashQrFrames = (ecash: string): string[] => dataToFrames(ecashToQrFrameData(ecash));
