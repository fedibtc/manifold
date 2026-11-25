import { Buffer } from 'buffer';
import { areFramesComplete, framesToData, parseFramesReducer, type State } from 'qrloop';
import { describe, expect, it } from 'vitest';
import { ecashQrFrames, ecashToQrFrameData } from '../ecashQrFrames';

// What the Fedi scanner does with the frames it reads: reassemble them, then
// turn binary back into base64 and leave text as it is
// (fedi `ui/common/utils/qrFrames.ts` `qrFrameDataToString`).
const scan = (frames: string[]): Buffer => {
  let state: State = null;
  for (const frame of frames) {
    state = parseFramesReducer(state, frame);
    if (areFramesComplete(state)) return framesToData(state);
  }
  throw new Error('the frames never completed');
};

// Long enough to need many frames, as a whole-balance token does.
const V2_TOKEN = `fedimint${'a1b2c3d4e5f6g7h8i9j0klmnopqrstuv'.repeat(40)}x`;
// Mint v1 tokens are URL-safe base64 with padding (fedimint `OOBNotes` Display);
// 3 001 bytes so the padding and both URL-safe characters are exercised.
const V1_BYTES = Buffer.from(Array.from({ length: 3_001 }, (_, at) => (at * 37 + 251) % 256));
const V1_TOKEN = V1_BYTES.toString('base64').replaceAll('+', '-').replaceAll('/', '_');

describe('ecashQrFrames', () => {
  it('should carry a mint-v2 token as text that scans back to the same string', () => {
    const frames = ecashQrFrames(V2_TOKEN);

    expect(frames.length).toBeGreaterThan(1);
    expect(scan(frames).toString('utf8')).toBe(V2_TOKEN);
  });

  it('should carry a mint-v1 token as its notes bytes', () => {
    const frames = ecashQrFrames(V1_TOKEN);

    expect(V1_TOKEN).toMatch(/[-_]/);
    expect(scan(frames).equals(V1_BYTES)).toBe(true);
  });

  // The scanner reads every frame in a loop, so it must complete from any
  // starting point, as a phone pointed at the screen mid-loop does.
  it('should scan back from a frame in the middle of the loop', () => {
    const frames = ecashQrFrames(V2_TOKEN);
    const half = Math.floor(frames.length / 2);

    expect(scan([...frames.slice(half), ...frames.slice(0, half)]).toString('utf8')).toBe(V2_TOKEN);
  });

  // A base64 decode would corrupt a v2 token: every one of its characters is
  // valid base64, so it decodes, to the wrong bytes.
  it('should not decode a mint-v2 token as base64', () => {
    expect(ecashToQrFrameData(V2_TOKEN).toString('utf8')).toBe(V2_TOKEN);
    expect(Buffer.from(V2_TOKEN, 'base64').toString('utf8')).not.toBe(V2_TOKEN);
  });
});
