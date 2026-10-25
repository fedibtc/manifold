#!/usr/bin/env python3
"""UDP/TCP fault-injection proxy for the guardian transfer QA env.

config JSON: {"control": path, "udp": [[listen, target], ...], "tcp": [[listen, target_port, group], ...]}
Control socket, one JSON command per line:
  {"cmd":"block","ranges":[[lo,hi],...],"tag":"x"}   drop UDP whose real target or source port is in a range
  {"cmd":"unblock","tag":"x"}
  {"cmd":"block_listen","ranges":[[lo,hi]],"tag":"x"}  drop UDP arriving on listen ports in range (a plane)
  {"cmd":"loss","tag":"x","ranges":[[lo,hi]],"p":0.3}  random loss
  {"cmd":"tcp_block","group":"g"} / {"cmd":"tcp_unblock","group":"g"}
  {"cmd":"retarget","listen":p,"target":q}
  {"cmd":"state"}
"""
import asyncio, json, random, sys, socket

cfg = json.load(open(sys.argv[1]))
blocks = {}      # tag -> list of (lo,hi) on real ports (target or source)
lblocks = {}     # tag -> list of (lo,hi) on listen ports
losses = {}      # tag -> (ranges, p)
tcp_blocked = set()
targets = {}     # listen -> target
stats = {"fwd": 0, "drop": 0}

def in_ranges(port, ranges):
    for lo, hi in ranges:
        if lo <= port <= hi:
            return True
    return False

def dropped(listen, target, src):
    for r in lblocks.values():
        if in_ranges(listen, r):
            return True
    for r in blocks.values():
        if in_ranges(target, r) or in_ranges(src, r):
            return True
    for r, p in losses.values():
        if (in_ranges(target, r) or in_ranges(src, r)) and random.random() < p:
            return True
    return False

class Upstream(asyncio.DatagramProtocol):
    def __init__(self, front, client):
        self.front, self.client = front, client
    def connection_made(self, t):
        self.t = t
    def datagram_received(self, data, addr):
        f = self.front
        if dropped(f.listen, targets[f.listen], self.client[1]):
            stats["drop"] += 1
            return
        f.t.sendto(data, self.client)
        stats["fwd"] += 1

class Front(asyncio.DatagramProtocol):
    def __init__(self, listen):
        self.listen = listen
        self.ups = {}
    def connection_made(self, t):
        self.t = t
    def datagram_received(self, data, addr):
        target = targets[self.listen]
        if dropped(self.listen, target, addr[1]):
            stats["drop"] += 1
            return
        up = self.ups.get(addr)
        if up is None or up[1] != target:
            if up is not None:
                up[0].close()
            loop = asyncio.get_running_loop()
            proto = Upstream(self, addr)
            tr = loop._create_upstream  # placeholder
        up = self.ups.get(addr)
        if up is None or up[1] != target:
            self.ups[addr] = None
            asyncio.ensure_future(self._open(addr, target, data))
            return
        up[0].sendto(data)
        stats["fwd"] += 1
    async def _open(self, addr, target, data):
        loop = asyncio.get_running_loop()
        tr, _ = await loop.create_datagram_endpoint(lambda: Upstream(self, addr), remote_addr=("127.0.0.1", target))
        old = self.ups.get(addr)
        if old:
            old[0].close()
        self.ups[addr] = (tr, target)
        tr.sendto(data)

# simpler: avoid placeholder attribute access
def _dr(self, data, addr):
    target = targets[self.listen]
    if dropped(self.listen, target, addr[1]):
        stats["drop"] += 1
        return
    up = self.ups.get(addr)
    if up is None or up[1] != target:
        if addr in self.pending:
            self.pending[addr].append(data)
            return
        self.pending[addr] = [data]
        asyncio.ensure_future(self._open2(addr, target))
        return
    up[0].sendto(data)
    stats["fwd"] += 1

async def _open2(self, addr, target):
    loop = asyncio.get_running_loop()
    tr, _ = await loop.create_datagram_endpoint(lambda: Upstream(self, addr), remote_addr=("127.0.0.1", target))
    old = self.ups.get(addr)
    if old:
        old[0].close()
    self.ups[addr] = (tr, target)
    for d in self.pending.pop(addr, []):
        tr.sendto(d)

Front.datagram_received = _dr
Front._open2 = _open2
_orig_init = Front.__init__
def _init(self, listen):
    _orig_init(self, listen)
    self.pending = {}
Front.__init__ = _init

tcp_conns = {}  # group -> set of writers

async def tcp_pipe(r, w):
    try:
        while True:
            d = await r.read(65536)
            if not d:
                break
            w.write(d)
            await w.drain()
    except Exception:
        pass
    finally:
        try: w.close()
        except Exception: pass

def tcp_handler(target, group):
    async def h(r, w):
        if group in tcp_blocked:
            w.close(); return
        try:
            ur, uw = await asyncio.open_connection("127.0.0.1", target)
        except Exception:
            w.close(); return
        tcp_conns.setdefault(group, set()).update([w, uw])
        await asyncio.gather(tcp_pipe(r, uw), tcp_pipe(ur, w))
        tcp_conns.get(group, set()).difference_update([w, uw])
    return h

async def control(r, w):
    while True:
        line = await r.readline()
        if not line:
            break
        try:
            c = json.loads(line)
            cmd = c["cmd"]
            if cmd == "block":
                blocks[c["tag"]] = [tuple(x) for x in c["ranges"]]
            elif cmd == "block_listen":
                lblocks[c["tag"]] = [tuple(x) for x in c["ranges"]]
            elif cmd == "loss":
                losses[c["tag"]] = ([tuple(x) for x in c["ranges"]], float(c["p"]))
            elif cmd == "unblock":
                blocks.pop(c["tag"], None); lblocks.pop(c["tag"], None); losses.pop(c["tag"], None)
            elif cmd == "clear":
                blocks.clear(); lblocks.clear(); losses.clear(); tcp_blocked.clear()
            elif cmd == "tcp_block":
                tcp_blocked.add(c["group"])
                for x in list(tcp_conns.get(c["group"], ())):
                    try: x.close()
                    except Exception: pass
            elif cmd == "tcp_unblock":
                tcp_blocked.discard(c["group"])
            elif cmd == "retarget":
                targets[int(c["listen"])] = int(c["target"])
            res = {"ok": True, "blocks": list(blocks), "lblocks": list(lblocks), "losses": list(losses),
                   "tcp_blocked": sorted(tcp_blocked), "stats": stats,
                   "retargeted": {k: v for k, v in targets.items() if k in c.get("show", [])}}
        except Exception as e:
            res = {"ok": False, "error": repr(e)}
        w.write((json.dumps(res) + "\n").encode()); await w.drain()

async def main():
    loop = asyncio.get_running_loop()
    for listen, target in cfg["udp"]:
        targets[listen] = target
        await loop.create_datagram_endpoint(lambda l=listen: Front(l), local_addr=("127.0.0.1", listen))
    for listen, target, group in cfg["tcp"]:
        await asyncio.start_server(tcp_handler(target, group), "127.0.0.1", listen)
    import os
    try: os.unlink(cfg["control"])
    except FileNotFoundError: pass
    await asyncio.start_unix_server(control, cfg["control"])
    print("netproxy ready", len(cfg["udp"]), "udp", len(cfg["tcp"]), "tcp", flush=True)
    await asyncio.Event().wait()

asyncio.run(main())
