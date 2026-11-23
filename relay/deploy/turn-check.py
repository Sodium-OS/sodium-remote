#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
"""TURN check for the relay (Python standard library only).

Over one transport to the server (udp, tcp or tls) it:
  1. allocates a UDP relay with the long-term credentials the rendezvous issued;
  2. learns a second local socket's public address (the "peer") with STUN;
  3. permits the peer, then relays packets peer -> relay -> client and
     client -> relay -> peer, timing each round trip;
  4. asks for permissions to private, loopback and link-local peers, which must be
     refused (403), unless --no-deny;
  5. deallocates (Refresh with lifetime 0).
With --bad-password it instead checks that a wrong password gets 401 and no relay.

This mirrors what ICE does with a relay candidate: one side relays, the other
sends to the relayed address from its own NAT mapping, so it doesn't need the
cloud NAT to hairpin (relay-to-relay on the same server does).

usage: turn-check.py <server> <udp|tcp|tls> <creds.json> [--ca FILE] [--port N]
                     [--stun-port N] [--count N] [--no-deny] [--bad-password]
                     [--own-ip IP] [--deny-peer IP ...]

--own-ip is the server's private relay address (coturn always grants it; the nft
guard must drop the traffic). --deny-peer adds an address of the server's own
networks that must be refused; repeat it for each.
"""
import argparse
import hashlib
import hmac
import ipaddress
import json
import os
import select
import socket
import ssl
import statistics
import struct
import sys
import time

COOKIE = 0x2112A442
BINDING, ALLOCATE, REFRESH, CREATE_PERMISSION = 0x001, 0x003, 0x004, 0x008
SEND_IND, DATA_IND = 0x0016, 0x0017
A_USERNAME, A_MI, A_ERROR, A_LIFETIME = 0x0006, 0x0008, 0x0009, 0x000D
A_XOR_PEER, A_DATA, A_REALM, A_NONCE = 0x0012, 0x0013, 0x0014, 0x0015
A_XOR_RELAYED, A_REQ_TRANSPORT, A_XOR_MAPPED = 0x0016, 0x0019, 0x0020
# Always probed: generic private, loopback, link-local and CGNAT addresses. Addresses
# of the server's own networks (its private relay address, VPN or VPC peers) are added
# with --own-ip and --deny-peer.
DENIED_PEERS = ["127.0.0.1", "10.0.0.1", "100.64.0.1", "169.254.0.23", "172.17.0.1", "192.168.1.1"]


def attr(t, v):
    return struct.pack("!HH", t, len(v)) + v + b"\0" * (-len(v) % 4)


def xor_addr(ip, port):
    x = int(ipaddress.IPv4Address(ip)) ^ COOKIE
    return struct.pack("!BBHI", 0, 1, port ^ (COOKIE >> 16), x)


def parse_xor_addr(v):
    _, fam, port, x = struct.unpack("!BBHI", v[:8])
    assert fam == 1, "IPv4 only"
    return str(ipaddress.IPv4Address(x ^ COOKIE)), port ^ (COOKIE >> 16)


def build(mtype, attrs=b"", key=None, tid=None):
    tid = tid or os.urandom(12)
    if key is None:
        return struct.pack("!HHI", mtype, len(attrs), COOKIE) + tid + attrs, tid
    hdr = struct.pack("!HHI", mtype, len(attrs) + 24, COOKIE) + tid
    mac = hmac.new(key, hdr + attrs, hashlib.sha1).digest()
    return hdr + attrs + attr(A_MI, mac), tid


def parse(msg):
    mtype, length, cookie = struct.unpack("!HHI", msg[:8])
    tid, attrs, i = msg[8:20], {}, 20
    while i < 20 + length:
        t, ln = struct.unpack("!HH", msg[i:i + 4])
        attrs.setdefault(t, msg[i + 4:i + 4 + ln])
        i += 4 + ln + (-ln % 4)
    return mtype, tid, attrs


def error_code(attrs):
    v = attrs.get(A_ERROR)
    return None if v is None else v[2] * 100 + v[3]


class Conn:
    """A STUN/TURN connection to the server over udp, tcp or tls."""

    def __init__(self, server, port, transport, ca):
        self.transport = transport
        self.addr = (socket.gethostbyname(server), port)
        if transport == "udp":
            self.s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            self.s.connect(self.addr)
        else:
            raw = socket.create_connection(self.addr, timeout=10)
            if transport == "tls":
                ctx = ssl.create_default_context(cafile=ca)
                raw = ctx.wrap_socket(raw, server_hostname=server)
                self.tls = f"{raw.version()} {raw.cipher()[0]}"
            self.s = raw
        self.s.settimeout(5)
        self.pending = []  # indications that arrived while waiting for a response

    def send(self, msg):
        self.s.sendall(msg) if self.transport != "udp" else self.s.send(msg)

    def recv(self, timeout=5):
        self.s.settimeout(timeout)
        if self.transport == "udp":
            return self.s.recv(65535)
        hdr = self._exact(20)
        return hdr + self._exact(struct.unpack("!H", hdr[2:4])[0])

    def _exact(self, n):
        b = b""
        while len(b) < n:
            chunk = self.s.recv(n - len(b))
            if not chunk:
                raise ConnectionError("server closed the connection")
            b += chunk
        return b

    def transact(self, msg, tid):
        for _ in range(3 if self.transport == "udp" else 1):
            self.send(msg)
            deadline = time.time() + 2
            while time.time() < deadline:
                try:
                    m = self.recv(max(0.05, deadline - time.time()))
                except socket.timeout:
                    break
                mtype, rtid, attrs = parse(m)
                if rtid == tid:
                    return mtype, attrs
                self.pending.append((mtype, attrs))
        raise TimeoutError("no response")


class Turn:
    def __init__(self, conn, username, password):
        self.c, self.user, self.pw = conn, username, password
        self.key = self.realm = self.nonce = None

    def request(self, method, attrs=b""):
        if self.key is None:  # first request: learn realm and nonce from the 401
            msg, tid = build(method, attrs)
            mtype, r = self.c.transact(msg, tid)
            if error_code(r) != 401:
                return mtype, r
            self.realm, self.nonce = r[A_REALM], r[A_NONCE]
            self.key = hashlib.md5(f"{self.user}:{self.realm.decode()}:{self.pw}".encode()).digest()
        for _ in range(2):  # retry once on a stale nonce (438)
            auth = attr(A_USERNAME, self.user.encode()) + attr(A_REALM, self.realm) + attr(A_NONCE, self.nonce)
            msg, tid = build(method, attrs + auth, self.key)
            mtype, r = self.c.transact(msg, tid)
            if error_code(r) == 438:
                self.nonce = r[A_NONCE]
                continue
            return mtype, r
        return mtype, r


def ok(cond, label):
    print(("ok    " if cond else "FAIL  ") + label)
    return cond


def relay_both_ways(conn, peer, relay, count):
    """peer -> relay -> client (Data indication), then client -> relay -> peer (Send
    indication), `count` times. Returns 1 on any loss, else 0."""
    rtts, lost = [], 0
    for i in range(count):
        payload = os.urandom(8) + struct.pack("!I", i) + b"sodium-remote-check" * 8
        t = time.time()
        peer.sendto(payload, relay)
        got = None
        deadline = t + 2
        while time.time() < deadline and got is None:
            while conn.pending and got is None:
                mt, at = conn.pending.pop(0)
                if mt == DATA_IND and at.get(A_DATA) == payload:
                    got = at
            if got is None:
                try:
                    mt, _, at = parse(conn.recv(max(0.05, deadline - time.time())))
                except socket.timeout:
                    break
                if mt == DATA_IND and at.get(A_DATA) == payload:
                    got = at
        if got is None:
            lost += 1
            continue
        src = parse_xor_addr(got[A_XOR_PEER])
        msg, _ = build(SEND_IND, attr(A_XOR_PEER, xor_addr(*src)) + attr(A_DATA, payload[::-1]))
        conn.send(msg)
        try:
            back = peer.recv(2048)
        except socket.timeout:
            lost += 1
            continue
        if back == payload[::-1]:
            rtts.append(1000 * (time.time() - t))
        else:
            lost += 1
    if rtts:
        print(f"      relay round trips: {len(rtts)}/{count}, median {statistics.median(rtts):.1f} ms, "
              f"min {min(rtts):.1f}, max {max(rtts):.1f}")
    return 0 if ok(lost == 0 and len(rtts) == count,
                   f"relayed {count} packets each way through the relay, {lost} lost") else 1


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("server")
    ap.add_argument("transport", choices=["udp", "tcp", "tls"])
    ap.add_argument("creds")
    ap.add_argument("--ca")
    ap.add_argument("--port", type=int, help="the TURN port for this transport (3478, or 5349 for tls)")
    ap.add_argument("--stun-port", type=int, default=3478, help="the server's UDP STUN port")
    ap.add_argument("--count", type=int, default=20)
    ap.add_argument("--no-deny", action="store_true")
    ap.add_argument("--bad-password", action="store_true")
    ap.add_argument("--own-ip", default=None, help="the server's private relay address (probed too)")
    ap.add_argument("--deny-peer", action="append", default=[], metavar="IP",
                    help="an extra address that must be refused (repeatable)")
    ap.add_argument("--hold", type=int, default=0,
                    help="keep refreshing the allocation this many seconds, then relay again")
    a = ap.parse_args()
    creds = json.load(open(a.creds))
    port = a.port or (5349 if a.transport == "tls" else 3478)
    conn = Conn(a.server, port, a.transport, a.ca)
    print(f"connected to {a.server}:{port} over {a.transport}" + (f" ({conn.tls})" if a.transport == "tls" else ""))
    turn = Turn(conn, creds["username"], "wrong" if a.bad_password else creds["credential"])
    req_udp = attr(A_REQ_TRANSPORT, bytes([17, 0, 0, 0]))

    t0 = time.time()
    mtype, r = turn.request(ALLOCATE, req_udp)
    if a.bad_password:
        good = ok(mtype == 0x0113 and error_code(r) == 401, f"wrong password refused: error {error_code(r)}")
        return 0 if good else 1
    if not ok(mtype == 0x0103, f"allocate: {'success' if mtype == 0x0103 else f'error {error_code(r)}'} "
                               f"in {1000 * (time.time() - t0):.0f} ms"):
        return 1
    relay = parse_xor_addr(r[A_XOR_RELAYED])
    mapped = parse_xor_addr(r[A_XOR_MAPPED])
    lifetime = struct.unpack("!I", r[A_LIFETIME])[0]
    print(f"      relayed {relay[0]}:{relay[1]}, client seen as {mapped[0]}:{mapped[1]}, lifetime {lifetime} s")
    failures = 0

    # The peer: a second socket here, its public address learned by STUN.
    peer = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    peer.settimeout(2)
    msg, tid = build(BINDING)
    peer.sendto(msg, (conn.addr[0], a.stun_port))
    _, _, pr = parse(peer.recv(2048))
    peer_pub = parse_xor_addr(pr[A_XOR_MAPPED])
    mtype, r = turn.request(CREATE_PERMISSION, attr(A_XOR_PEER, xor_addr(*peer_pub)))
    failures += not ok(mtype == 0x0108, f"permission for the peer {peer_pub[0]}:{peer_pub[1]}"
                                         + ("" if mtype == 0x0108 else f": error {error_code(r)}"))
    failures += relay_both_ways(conn, peer, relay, a.count)

    if a.hold:
        # Keep the allocation past the credential's expiry, the way a long session would.
        start, end = time.time(), time.time() + a.hold
        expiry = int(creds["username"].split(":")[0])
        while time.time() < end:
            time.sleep(min(100, max(0, end - time.time())))
            mt, r = turn.request(REFRESH, attr(A_LIFETIME, struct.pack("!I", 600)))
            mt2, r2 = turn.request(CREATE_PERMISSION, attr(A_XOR_PEER, xor_addr(*peer_pub)))
            left = expiry - int(time.time())
            cred = f"expires in {left} s" if left > 0 else f"expired {-left} s ago"
            failures += not ok(mt == 0x0104 and mt2 == 0x0108,
                               f"t+{time.time() - start:.0f} s (credential {cred}): "
                               f"refresh {'ok' if mt == 0x0104 else error_code(r)}, "
                               f"permission {'ok' if mt2 == 0x0108 else error_code(r2)}")
        failures += relay_both_ways(conn, peer, relay, a.count)

    if not a.no_deny:
        probes = list(DENIED_PEERS)
        for extra in ([a.own_ip] if a.own_ip else []) + a.deny_peer:
            ipaddress.IPv4Address(extra)
            if extra not in probes:
                probes.append(extra)
        for ip in probes:
            mtype, r = turn.request(CREATE_PERMISSION, attr(A_XOR_PEER, xor_addr(ip, 22)))
            if ip == a.own_ip and mtype == 0x0108:
                # coturn always grants its own relay address (relay-to-relay goes there);
                # the nft egress guard must drop anything but its relay ports. Send one
                # packet to port 22 for the guard's counter.
                conn.send(build(SEND_IND, attr(A_XOR_PEER, xor_addr(ip, 22)) + attr(A_DATA, b"guard probe"))[0])
                print(f"note  permission for {ip} (the server's own relay address) granted by coturn; "
                      "sent one probe packet for the egress guard to drop")
                continue
            failures += not ok(mtype == 0x0118 and error_code(r) == 403,
                               f"permission for {ip} refused: "
                               + ("error 403" if error_code(r) == 403 else
                                  f"GRANTED ({mtype:#06x}, error {error_code(r)})"))

    mtype, r = turn.request(REFRESH, attr(A_LIFETIME, struct.pack("!I", 0)))
    failures += not ok(mtype == 0x0104, "deallocated")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
