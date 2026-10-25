
import hashlib, json, os, signal, subprocess, sys, time, socket
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import nostr_sign

Q = os.path.dirname(os.path.abspath(__file__))
BIN = os.environ.get('QA_BIN', '/src/qa-transfer-manifold-rb/target/debug')
MG_COUNT = 14
MAX_SEATS = 20
PORT_BASE = 26000
CORE = 16000
FAP = 14000
RELAY_PX = 13000
PUBLISHER_SECRET = 3
PUBLISHER = nostr_sign.xonly(PUBLISHER_SECRET).hex()
SETUP_D_TAG = 'setup-payment-federations'

def defe_env():
    out = {}
    for line in open(os.path.join(Q, 'defe.env')):
        if line.startswith('DEV_DEFE_') and '=' in line:
            k, v = line.strip().split('=', 1)
            out[k] = v
    return out

DE = defe_env()
RELAY = DE['DEV_DEFE_NOSTR_RELAY_URL']
RELAY_PORT = int(DE['DEV_DEFE_NOSTR_RELAY_PORT'])
BTC = {k: DE['DEV_DEFE_BITCOIND_' + k] for k in ['URL', 'RPC_HOST', 'RPC_PORT', 'RPC_USERNAME', 'RPC_PASSWORD']}

def run(cmd, env=None, timeout=120, check=True):
    r = subprocess.run(cmd, env={**os.environ, **(env or {})}, capture_output=True, text=True, timeout=timeout)
    if check and r.returncode != 0:
        raise RuntimeError(f'{cmd[:4]} failed ({r.returncode}):\n{r.stdout[-3000:]}\n{r.stderr[-3000:]}')
    return r.stdout.strip()

def btc(*args, wallet=None):
    cmd = ['bitcoin-cli', f'-rpcconnect={BTC["RPC_HOST"]}', f'-rpcport={BTC["RPC_PORT"]}',
           f'-rpcuser={BTC["RPC_USERNAME"]}', f'-rpcpassword={BTC["RPC_PASSWORD"]}']
    if wallet: cmd.append(f'-rpcwallet={wallet}')
    return run(cmd + list(args))

def mg_base(g): return PORT_BASE + g * 100
def seat_port(g, s, role): return mg_base(g) + s * 4 + (0 if role == 'p2p' else 1)
def endpoint_port(g): return mg_base(g) + 99

_idcache = {}
def iroh_id(port, role):
    k = (port, role)
    if k not in _idcache:
        secret = hashlib.sha256(b'fman-e2e-local-iroh-v1\0' + port.to_bytes(2, 'big') + role.encode()).digest()
        _idcache[k] = nostr_sign.ed25519_public(secret).hex()
    return _idcache[k]

def plane_port(plane, real):
    return real - PORT_BASE + plane

def overrides(plane, moved=None):
    """plane: CORE/FAP or None for direct. moved: {(g,s): (d,t)} for direct plane."""
    out = []
    for g in range(MG_COUNT):
        for s in range(MAX_SEATS):
            for role in ('p2p', 'api'):
                real = seat_port(g, s, role)
                if plane is None:
                    if moved and (g, s) in moved:
                        d, t = moved[(g, s)]
                        real_t = seat_port(d, t, role)
                    else:
                        real_t = real
                    out.append(f'{iroh_id(real, role)}=127.0.0.1:{real_t}')
                else:
                    out.append(f'{iroh_id(real, role)}=127.0.0.1:{plane_port(plane, real)}')
    return ','.join(out)

def proxy_config():
    udp = []
    for plane in (CORE, FAP):
        for g in range(MG_COUNT):
            for s in range(MAX_SEATS):
                for role in ('p2p', 'api'):
                    real = seat_port(g, s, role)
                    udp.append([plane_port(plane, real), real])
            udp.append([plane_port(plane, endpoint_port(g)), endpoint_port(g)])
    tcp = [[RELAY_PX + g, RELAY_PORT, f'relay:mg{g}'] for g in range(MG_COUNT)]
    tcp += [[RELAY_PX + 90 + i, RELAY_PORT, f'relay:fa{i}'] for i in range(5)]
    tcp += [[RELAY_PX + 99, RELAY_PORT, 'relay:misc']]
    return {'control': os.path.join(Q, 'proxy.sock'), 'udp': udp, 'tcp': tcp}

def proxy(cmd):
    s = socket.socket(socket.AF_UNIX)
    s.connect(os.path.join(Q, 'proxy.sock'))
    s.sendall((json.dumps(cmd) + '\n').encode())
    buf = b''
    while not buf.endswith(b'\n'):
        buf += s.recv(65536)
    s.close()
    return json.loads(buf)

def mg_relay(g): return f'ws://127.0.0.1:{RELAY_PX + g}'
def fa_relay(i=0): return f'ws://127.0.0.1:{RELAY_PX + 90 + i}'

class Env:
    def __init__(self, root):
        self.root = root
        os.makedirs(root, exist_ok=True)
        self.state_path = os.path.join(root, 'state.json')
        self.st = json.load(open(self.state_path)) if os.path.exists(self.state_path) else {'locators': {}, 'pids': {}, 'mg_bin': {}}
        self.procs = {}

    def save(self):
        json.dump(self.st, open(self.state_path + '.tmp', 'w'), indent=1)
        os.replace(self.state_path + '.tmp', self.state_path)

    def mg_dir(self, g): return os.path.join(self.root, f'mg-{g}')

    def endpoint_routes(self, plane):
        pairs = []
        for g, loc in sorted(self.st['locators'].items(), key=lambda x: int(x[0])):
            eid = json.loads(loc)['endpoint_addr']['id']
            pairs.append(f'{eid}=127.0.0.1:{plane_port(plane, endpoint_port(int(g)))}')
        return ','.join(pairs)

    def mg_env(self, g):
        return {
            'MANIFOLD_DEV_NOSTR_RELAYS': mg_relay(g),
            'MANIFOLD_DEV_SETUP_PAYMENT_PUBLISHER': PUBLISHER,
            'MG_E2E_LOCAL_IROH': '1',
            'FM_IROH_CONNECT_OVERRIDES_PLAIN': overrides(CORE),
            'MG_E2E_MG_ENDPOINT_ADDRS': self.endpoint_routes(CORE),
            'MG_E2E_IROH_BIND_ADDR': f'127.0.0.1:{endpoint_port(g)}',
            'RUST_LOG': os.environ.get('QA_MG_RUST_LOG', 'info'),
            'RUST_BACKTRACE': '1',
        }

    def mg_pid(self, g):
        p = self.st['pids'].get(str(g))
        if p and os.path.exists(f'/proc/{p}') and open(f'/proc/{p}/stat').read().split()[2] != 'Z':
            return p
        return None

    def start_mg(self, g, bin_dir=None, extra_env=None):
        assert self.mg_pid(g) is None, f'mg-{g} already runs'
        bin_dir = bin_dir or self.st['mg_bin'].get(str(g)) or BIN
        d = self.mg_dir(g)
        os.makedirs(d, exist_ok=True)
        logf = open(os.path.join(d, 'stdout.log'), 'a')
        logf.write(f'\n===== start {time.strftime("%H:%M:%S")} bin={bin_dir}\n'); logf.flush()
        c = subprocess.Popen([f'{bin_dir}/fleet-manager', 'serve', '--data-dir', d, '--manifold-environment', 'development',
                              '--bitcoind-url', BTC['URL'], '--bitcoind-username', BTC['RPC_USERNAME'],
                              '--bitcoind-password', BTC['RPC_PASSWORD'], '--first-port-base', str(mg_base(g))],
                             env={**os.environ, **self.mg_env(g), **(extra_env or {})}, stdout=logf, stderr=subprocess.STDOUT,
                             stdin=subprocess.DEVNULL, start_new_session=True)
        self.procs[g] = c
        self.st['pids'][str(g)] = c.pid
        self.save()
        return c

    def children(self, pid):
        out = []
        try:
            for t in os.listdir(f'/proc/{pid}/task'):
                for c in open(f'/proc/{pid}/task/{t}/children').read().split():
                    out.append(int(c))
        except FileNotFoundError:
            pass
        return out

    def seat_children(self, g):
        p = self.mg_pid(g)
        return self.children(p) if p else []

    def stop_mg(self, g, sig=signal.SIGINT, wait=30):
        p = self.mg_pid(g)
        if not p:
            return
        kids = self.children(p)
        os.kill(p, sig)
        deadline = time.time() + wait
        while time.time() < deadline and self.mg_pid(g):
            if g in self.procs: self.procs[g].poll()
            time.sleep(0.2)
        if self.mg_pid(g):
            os.kill(p, signal.SIGKILL)
            time.sleep(0.5)
            if g in self.procs: self.procs[g].poll()
        if sig == signal.SIGKILL:
            # a SIGKILLed MG leaves its children (they get PDEATHSIG maybe); report
            pass
        return kids

    def kill_mg(self, g, kill_children=True):
        p = self.mg_pid(g)
        kids = self.children(p) if p else []
        if p:
            os.kill(p, signal.SIGKILL)
        if kill_children:
            for k in kids:
                try: os.kill(k, signal.SIGKILL)
                except ProcessLookupError: pass
        time.sleep(0.5)
        if g in self.procs: self.procs[g].poll()
        return kids

    def fman(self, g, *args, timeout=60, check=True):
        bin_dir = self.st['mg_bin'].get(str(g)) or BIN
        out = run([f'{bin_dir}/fman-cli', '--data-dir', self.mg_dir(g), *args], timeout=timeout, check=check)
        try:
            return json.loads(out) if out else None
        except json.JSONDecodeError:
            return out

    def read_locator(self, g, timeout=120):
        path = os.path.join(self.mg_dir(g), 'stdout.log')
        deadline = time.time() + timeout
        while time.time() < deadline:
            found = None
            for line in open(path, errors='replace'):
                if line.startswith('Fleet Manager locator: '):
                    found = line[len('Fleet Manager locator: '):].strip()
            if found:
                return found
            time.sleep(0.5)
        raise RuntimeError(f'mg-{g} printed no locator')

    def onboard(self, g, max_seats=MAX_SEATS, price=0):
        d = self.mg_dir(g)
        deadline = time.time() + 60
        while True:
            r = subprocess.run([f'{BIN}/fman-cli', '--data-dir', d, 'onboard', 'new'], capture_output=True, text=True)
            if r.returncode == 0: break
            if time.time() > deadline: raise RuntimeError(r.stderr)
            time.sleep(0.3)
        ob = self.fman(g, 'onboarding')
        if ob['stage'] == 'holder_authorization':
            run([f'{BIN}/manifold-test-issuer', '--environment', 'development', '--relay', RELAY,
                 '--authorization-request', json.dumps({'subject_pubkey': ob['service_nostr_pubkey']}),
                 '--publish-mg-authorization'])
            self.fman(g, 'refresh-holder-authorizations')
        self.fman(g, 'onboard', 'offer', '--max-seats', str(max_seats), '--price-msats', str(price))

    def fa_cli_env(self, plane=FAP, relay_i=4):
        return {'MG_E2E_LOCAL_IROH': '1', 'MG_E2E_MG_ENDPOINT_ADDRS': self.endpoint_routes(plane),
                'MANIFOLD_DEV_NOSTR_RELAYS': fa_relay(relay_i), 'FM_IROH_CONNECT_OVERRIDES_PLAIN': overrides(plane)}
