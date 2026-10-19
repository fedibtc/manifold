import { QRCodeSVG } from 'qrcode.react';
import styles from './AuthorizationQrCode.module.css';

interface AuthorizationQrCodeProps {
  request: string;
  size: number;
}

export const AuthorizationQrCode = ({ request, size }: AuthorizationQrCodeProps) => (
  <div className={styles.qrBox}>
    <QRCodeSVG value={request} size={size} marginSize={2} />
  </div>
);
