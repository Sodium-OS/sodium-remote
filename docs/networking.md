# Networking

How Sodium Remote finds another computer and connects to it, and what your network has to allow. Nothing here needs port forwarding.

## The short version

| Where the two computers are | What happens | Needs a server? |
|---|---|---|
| On the same network | The computers find each other with mDNS, pair and connect directly | No |
| You know the other one's address | You type `address[:port]`; pairing and connecting go straight to it | No |
| On different networks | Both reach a **rendezvous server**, exchange connection candidates, try to connect directly (hole punching), and fall back to a **relay** | Yes: yours |

A server only introduces the computers and relays when it must. It never sees the content: everything is encrypted end to end ([security](security.md)).

## Discovery on the local network

- A computer that is allowed to be controlled advertises itself over mDNS (DNS-SD) as `_sodium-remote._tcp`. It is on by default, and on SodiumOS a setting turns it off. The record carries the computer's name, a start of its ID and whether it is open for pairing.
- The app lists computers it finds under "Found nearby · not paired yet" and shows paired ones as online.
- On macOS the app needs the **Local Network** permission to browse.
- Pairing and signalling on the local network go over a TCP **LAN listener**: port **47311**, or any free port if that is taken. It is how a computer pairs and negotiates a connection without any server.
- mDNS uses UDP 5353 multicast. Networks that block multicast between devices (some guest Wi-Fi, some corporate networks) hide computers from each other; connect by address instead.

## Connecting by address

Type the other computer's name or IP address, with a port if it isn't 47311 (for example `192.0.2.10:47311`). Pairing by address and connecting by address use the same LAN listener. The address must be reachable from where you are: on the same network, over a VPN, or because you forwarded the port yourself. You never have to forward ports for the other methods.

## Across the internet: candidates, hole punching and relays

When a rendezvous server is configured, two computers connect like this:

1. Both stay connected to the server over TLS (a WebSocket on port 8443 by default). A host being "online" means it holds that connection.
2. A client asks to connect. The server tells the host. The two exchange **candidates**, sealed end to end, through the server: their local addresses, their addresses as the internet sees them (found with STUN), and relayed addresses (TURN) in case nothing else works.
3. They test every pair of candidates (ICE, RFC 8445) and use the best one that works. **Hole punching** is this step: each side sends to the other at the same time, which opens a way through home routers and most NATs without any forwarding. If both are on the same network, the pair is a local one; on different networks it is direct through the NATs.
4. When no direct path exists (for example, both behind a symmetric NAT, or UDP is blocked), traffic goes through a **relay**: a TURN server on the rendezvous server's side, over UDP, TCP or TLS. TURN over TLS on port 5349 gets through firewalls that allow only HTTPS-like traffic.

The relay only forwards encrypted packets. Its capacity is a real ceiling, so a relayed session can be slower than a direct one.

### Choosing and switching paths

- Media takes the first working path immediately, and then moves to a better one when it finds one.
- Paths are ranked by the lowest round-trip time over the last 10 seconds, with a penalty for each relay leg (UDP relay small, TCP and TLS relay larger), for loss above 2%, and, on the side that sends video, for too little throughput.
- A working direct or UDP-relay path is never left for a TCP or TLS relay; only its failure moves media there.
- A direct path that becomes possible later (for example after a firewall change) is found within about five seconds, and media moves to it without a new handshake or keyframe.
- If the active path stops answering, media switches to another within about 0.7 seconds, and the session continues. When your network changes (Wi-Fi to Ethernet, a VPN connecting), the computer gathers new candidates and reconnects.
- With several relay regions configured on the server, the client measures each and uses the fastest ([self-hosting](self-hosting.md)).

The app shows how you're connected: **LAN**, **Direct** or **Relay**, with the round-trip time.

### Without a server

With no server configured, computers still work on a local network and by address, but there are no STUN or relay servers to use: connections are direct only, and a computer behind a NAT can't be reached from outside unless you reach it by address (a VPN, or a port you forwarded yourself). Settings › Server says so. See [the user guide](user-guide.md) for adding a server.

## Ports and firewalls

### On the computers

| What | Port | Notes |
|---|---|---|
| LAN listener, pairing and signalling on the local network | TCP 47311 (or a free port) | Inbound, on computers that can be controlled |
| mDNS discovery | UDP 5353, multicast | Inbound and outbound on the local network |
| Media | UDP, ports chosen at run time | Between the two computers when connected directly; outbound only is enough when a NAT or the relay is in between |
| To the server | TCP 8443 outbound (or whatever the server's URL says), UDP/TCP 3478 and TCP 5349 outbound | Only when a server is configured |

Windows computers must treat the network as Private. A host-side firewall must let the app listen on the LAN listener.

### On a server you run

| Port | Protocol | Used for |
|---|---|---|
| 8443 | TCP | The rendezvous server (HTTPS and WebSocket) |
| 3478 | UDP and TCP | STUN and TURN |
| 5349 | TCP | TURN over TLS |
| 50000–50099 | UDP | Relayed addresses (a range of your choice, one port per allocation) |

All are adjustable. Open them inbound on the server's firewall. [Self-hosting](self-hosting.md) has the details.

## Stability

- A session survives short network drops: the host holds the session for about two minutes and the client keeps retrying ("Reconnecting…", then "Waiting for the network…" when there is no route).
- If a host's network address changes, its clients find it again through the server, or by mDNS on the local network.

For the protocol itself, see [the protocol reference](reference/protocol.md) and [the rendezvous API](reference/rendezvous-api.md).
