"""Minimal ed25519 public keys, BIP340 signing and a Nostr publish, for the local FA E2E env only."""
import base64, hashlib, json, os, socket, struct, time
from urllib.parse import urlparse

# ---- ed25519 (public key derivation only) ----
_p = 2**255 - 19
_d = -121665 * pow(121666, _p - 2, _p) % _p
_I = pow(2, (_p - 1) // 4, _p)
def _xrecover(y):
    xx = (y * y - 1) * pow(_d * y * y + 1, _p - 2, _p)
    x = pow(xx, (_p + 3) // 8, _p)
    if (x * x - xx) % _p != 0: x = x * _I % _p
    if x % 2 != 0: x = _p - x
    return x
_By = 4 * pow(5, _p - 2, _p) % _p
_B = (_xrecover(_By), _By, 1, _xrecover(_By) * _By % _p)
def _add(P, Q):
    x1, y1, z1, t1 = P; x2, y2, z2, t2 = Q
    a = (y1 - x1) * (y2 - x2) % _p; b = (y1 + x1) * (y2 + x2) % _p
    c = t1 * 2 * _d * t2 % _p; dd = z1 * 2 * z2 % _p
    e, f, g, h = b - a, dd - c, dd + c, b + a
    return (e * f % _p, g * h % _p, f * g % _p, e * h % _p)
def _mul(P, n):
    Q = (0, 1, 1, 0)
    while n:
        if n & 1: Q = _add(Q, P)
        P = _add(P, P); n >>= 1
    return Q
def ed25519_public(secret32: bytes) -> bytes:
    h = hashlib.sha512(secret32).digest()
    a = int.from_bytes(h[:32], 'little')
    a &= (1 << 254) - 8; a |= 1 << 254
    x, y, z, _ = _mul(_B, a)
    zi = pow(z, _p - 2, _p); x = x * zi % _p; y = y * zi % _p
    return (y | ((x & 1) << 255)).to_bytes(32, 'little')

# ---- secp256k1 BIP340 ----
P = 2**256 - 2**32 - 977
N = 0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEBAAEDCE6AF48A03BBFD25E8CD0364141
G = (0x79BE667EF9DCBBAC55A06295CE870B07029BFCDB2DCE28D959F2815B16F81798,
     0x483ADA7726A3C4655DA4FBFC0E1108A8FD17B448A68554199C47D08FFB10D4B8)
def _padd(a, b):
    if a is None: return b
    if b is None: return a
    if a[0] == b[0] and a[1] != b[1]: return None
    if a == b: lam = 3 * a[0] * a[0] * pow(2 * a[1], P - 2, P) % P
    else: lam = (b[1] - a[1]) * pow(b[0] - a[0], P - 2, P) % P
    x = (lam * lam - a[0] - b[0]) % P
    return (x, (lam * (a[0] - x) - a[1]) % P)
def _pmul(pt, n):
    r = None
    for i in range(256):
        if (n >> i) & 1: r = _padd(r, pt)
        pt = _padd(pt, pt)
    return r
def _th(tag, data):
    t = hashlib.sha256(tag.encode()).digest()
    return hashlib.sha256(t + t + data).digest()
def xonly(seckey: int) -> bytes:
    return _pmul(G, seckey)[0].to_bytes(32, 'big')
def schnorr_sign(msg: bytes, seckey: int) -> bytes:
    Pt = _pmul(G, seckey)
    d = seckey if Pt[1] % 2 == 0 else N - seckey
    aux = os.urandom(32)
    t = (d ^ int.from_bytes(_th('BIP0340/aux', aux), 'big')).to_bytes(32, 'big')
    k0 = int.from_bytes(_th('BIP0340/nonce', t + Pt[0].to_bytes(32, 'big') + msg), 'big') % N
    R = _pmul(G, k0)
    k = k0 if R[1] % 2 == 0 else N - k0
    e = int.from_bytes(_th('BIP0340/challenge', R[0].to_bytes(32, 'big') + Pt[0].to_bytes(32, 'big') + msg), 'big') % N
    return R[0].to_bytes(32, 'big') + ((k + e * d) % N).to_bytes(32, 'big')

def sign_event(seckey: int, kind: int, tags, content: str):
    pub = xonly(seckey).hex()
    created = int(time.time())
    ser = json.dumps([0, pub, created, kind, tags, content], separators=(',', ':'), ensure_ascii=False)
    eid = hashlib.sha256(ser.encode()).digest()
    return {'id': eid.hex(), 'pubkey': pub, 'created_at': created, 'kind': kind, 'tags': tags,
            'content': content, 'sig': schnorr_sign(eid, seckey).hex()}

# ---- websocket publish ----
def _ws_send(sock, text):
    data = text.encode(); mask = os.urandom(4)
    hdr = bytes([0x81])
    n = len(data)
    if n < 126: hdr += bytes([0x80 | n])
    elif n < 65536: hdr += bytes([0x80 | 126]) + struct.pack('>H', n)
    else: hdr += bytes([0x80 | 127]) + struct.pack('>Q', n)
    sock.sendall(hdr + mask + bytes(b ^ mask[i % 4] for i, b in enumerate(data)))
def _ws_recv(sock):
    def rd(n):
        b = b''
        while len(b) < n:
            c = sock.recv(n - len(b))
            if not c: raise EOFError
            b += c
        return b
    h = rd(2); n = h[1] & 0x7f
    if n == 126: n = struct.unpack('>H', rd(2))[0]
    elif n == 127: n = struct.unpack('>Q', rd(8))[0]
    return rd(n).decode()
def publish(relay_url: str, event: dict, timeout=10):
    u = urlparse(relay_url)
    sock = socket.create_connection((u.hostname, u.port or 80), timeout=timeout)
    key = base64.b64encode(os.urandom(16)).decode()
    sock.sendall((f'GET {u.path or "/"} HTTP/1.1\r\nHost: {u.hostname}:{u.port}\r\nUpgrade: websocket\r\n'
                  f'Connection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n').encode())
    resp = b''
    while b'\r\n\r\n' not in resp: resp += sock.recv(1)
    assert b' 101 ' in resp.split(b'\r\n')[0], resp
    _ws_send(sock, json.dumps(['EVENT', event]))
    while True:
        msg = json.loads(_ws_recv(sock))
        if msg[0] == 'OK' and msg[1] == event['id']:
            sock.close()
            if not msg[2]: raise RuntimeError(f'relay rejected event: {msg}')
            return
