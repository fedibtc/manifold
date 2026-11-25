import { QRCodeSVG } from 'qrcode.react';
import { useEffect, useState } from 'react';
import { ecashQrFrames } from '@/features/payouts/utils/ecashQrFrames';
import styles from './EcashQrCode.module.css';

interface EcashQrCodeProps {
  token: string;
}

// Up to this length one code stays easy for a phone camera to read off a
// screen. A token grows with its notes; past this it is shown as a loop of
// smaller codes the Fedi scanner reassembles.
export const MAX_SINGLE_QR_TOKEN_LENGTH = 600;

/** How long each frame of a looping code shows, as in the Fedi app's own send
 *  screen. */
export const QR_FRAME_MS = 100;

const QR_SIZE_PX = 320;

/** One code for a short token, a loop of qrloop frames for a long one. */
export const EcashQrCode = ({ token }: EcashQrCodeProps) => {
  const [frames] = useState(() =>
    token.length > MAX_SINGLE_QR_TOKEN_LENGTH ? ecashQrFrames(token) : [token]
  );
  const [index, setIndex] = useState(0);

  useEffect(() => {
    if (frames.length < 2) return;
    const timer = setInterval(() => setIndex((at) => (at + 1) % frames.length), QR_FRAME_MS);
    return () => clearInterval(timer);
  }, [frames]);

  return (
    <div className={styles.box}>
      <QRCodeSVG
        value={frames[index] ?? token}
        size={QR_SIZE_PX}
        level="L"
        marginSize={2}
        title={frames.length > 1 ? 'Ecash QR code, animated' : 'Ecash QR code'}
      />
    </div>
  );
};
