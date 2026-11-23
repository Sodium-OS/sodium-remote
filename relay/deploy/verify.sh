#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# Verifies the deployed relay from this machine and saves one log per check:
# TLS chains, the health endpoint, STUN, the rendezvous API end to end (a test host
# enrolled for the run and removed afterwards), TURN allocations over UDP, TCP and
# TLS with credentials the rendezvous issued, refused credentials and refused peers.
#
#   SODIUM_REMOTE_SSH=admin@relay.example.com SODIUM_REMOTE_SERVER_NAME=relay.example.com \
#     ./verify.sh <out-dir>
#
# Environment (same names as deploy.sh, which this script calls):
#   SODIUM_REMOTE_SSH, SODIUM_REMOTE_SERVER_NAME   required
#   SODIUM_REMOTE_SSH_KEY, SODIUM_REMOTE_SSH_PORT  optional SSH settings
#   SODIUM_REMOTE_PORT (8443), SODIUM_REMOTE_TURN_PORT (3478), SODIUM_REMOTE_TURNS_PORT (5349)
#   SODIUM_REMOTE_ENROL        open | token | allowlist (token): how the test host is enrolled;
#                              in token mode a one-use token is minted and exported to the
#                              probe as SODIUM_REMOTE_ENROL_TOKEN
#   SODIUM_REMOTE_TLS_MODE     provided (default) trusts SODIUM_REMOTE_CA_BUNDLE
#                              (/etc/ssl/certs/ca-certificates.crt); private-ca fetches the
#                              server's CA with deploy.sh fetch-ca and trusts only that
#   SODIUM_REMOTE_PRIVATE_IP   optional: the server's private relay address, probed too (it is
#                              granted by coturn; the nft guard must drop it)
#   SODIUM_REMOTE_DENY_PEERS   optional: space-separated extra addresses of the server's own
#                              networks (VPC, VPN) that TURN must refuse
#
# Needs: turnutils_uclient/turnutils_stunclient (the coturn package; never run its
# service here), jq, openssl, curl, cargo.
set -uo pipefail
OUT=${1:?usage: verify.sh <out-dir> (with SODIUM_REMOTE_SSH and SODIUM_REMOTE_SERVER_NAME set)}
NAME=${SODIUM_REMOTE_SERVER_NAME:?SODIUM_REMOTE_SERVER_NAME is required, e.g. relay.example.com}
: "${SODIUM_REMOTE_SSH:?SODIUM_REMOTE_SSH is required, e.g. admin@relay.example.com}"
RDV_PORT=${SODIUM_REMOTE_PORT:-8443}
TURN_PORT=${SODIUM_REMOTE_TURN_PORT:-3478}
TURNS_PORT=${SODIUM_REMOTE_TURNS_PORT:-5349}
ENROL=${SODIUM_REMOTE_ENROL:-token}
TLS_MODE=${SODIUM_REMOTE_TLS_MODE:-provided}
export SODIUM_REMOTE_SERVER_NAME SODIUM_REMOTE_SSH SODIUM_REMOTE_ENROL=$ENROL
HERE=$(cd "$(dirname "$0")" && pwd)
WORKSPACE=$(cd "$HERE/../.." && pwd)
mkdir -p "$OUT"
work=$(mktemp -d)
chmod 700 "$work"
host_id=""
cleanup() {
  [ -n "$host_id" ] && [ "$ENROL" = allowlist ] && "$HERE/deploy.sh" disallow-host "$host_id" >/dev/null
  rm -rf "$work"
}
trap cleanup EXIT

ip=$(getent ahostsv4 "$NAME" | awk 'NR == 1 { print $1 }')
pass=0
fail=0
check() { # check <name> <command...>: PASS when the command succeeds; log in $OUT/<name>.txt
  local name=$1
  shift
  if "$@" >"$OUT/$name.txt" 2>&1; then
    echo "PASS  $name"
    pass=$((pass + 1))
  else
    echo "FAIL  $name (see $OUT/$name.txt)"
    fail=$((fail + 1))
  fi
}

if [ "$TLS_MODE" = private-ca ]; then
  "$HERE/deploy.sh" fetch-ca "$work/ca.crt" >/dev/null || { echo "could not fetch the CA certificate"; exit 1; }
  cp "$work/ca.crt" "$OUT/private-ca.crt"
else
  cp "${SODIUM_REMOTE_CA_BUNDLE:-/etc/ssl/certs/ca-certificates.crt}" "$work/ca.crt" \
    || { echo "no CA bundle: set SODIUM_REMOTE_CA_BUNDLE"; exit 1; }
fi

tls_chain() { # host port: the served chain verifies against the trusted CAs for NAME
  local o
  o=$(openssl s_client -connect "$1:$2" -servername "$NAME" -verify_hostname "$NAME" \
        -CAfile "$work/ca.crt" -verify_return_error -alpn http/1.1 </dev/null 2>&1)
  echo "$o" | grep -E 'subject=|issuer=|Protocol|Cipher is|ALPN|Verify return code'
  grep -q 'Verify return code: 0 (ok)' <<<"$o"
}

healthz() {
  local o
  o=$(curl -sS --max-time 10 --cacert "$work/ca.crt" -w 'http %{http_code}, %{time_total}s\n' \
        "https://$NAME:$RDV_PORT/healthz")
  echo "$o"
  [[ $o == ok* ]]
}

stun_udp() {
  local o
  o=$(turnutils_stunclient -p "$TURN_PORT" "$ip" 2>&1)
  echo "$o"
  grep -q 'UDP reflexive addr' <<<"$o"
}

rendezvous() {
  local probe="$WORKSPACE/target/release/examples/probe"
  cargo build --release --example probe -p remote-rendezvous --manifest-path "$WORKSPACE/Cargo.toml" -q || return 1
  "$probe" keygen "$work/keys" >/dev/null || return 1
  # The test host gets a key in remoted's format (Ed25519, PKCS#8 PEM) and is enrolled
  # the way a user enrols theirs: allow-host with no argument reads the key file.
  openssl genpkey -algorithm ed25519 -out "$work/device.key" 2>/dev/null || return 1
  openssl pkey -in "$work/device.key" -outform DER | tail -c 32 | base64 -w0 | tr '+/' '-_' | tr -d '=' \
    > "$work/keys/host.key"
  host_id=$(SODIUM_REMOTE_HOST_KEY="$work/device.key" "$HERE/deploy.sh" host-id | head -n1) || return 1
  case $ENROL in
    allowlist) SODIUM_REMOTE_HOST_KEY="$work/device.key" "$HERE/deploy.sh" allow-host || return 1 ;;
    token)
      SODIUM_REMOTE_ENROL_TOKEN=$("$HERE/deploy.sh" token-new --uses 1 --expires 30m | grep -oE '[A-Za-z0-9_.-]{16,}' | tail -1) \
        || return 1
      export SODIUM_REMOTE_ENROL_TOKEN ;;
  esac
  "$probe" run "wss://$NAME:$RDV_PORT/remote" "$work/ca.crt" "$work/keys" "$work/turn.json"
}

sleep_notice() { # the host's sleep notice shows it asleep (with since) until it comes back
  "$WORKSPACE/target/release/examples/probe" sleep-check "wss://$NAME:$RDV_PORT/remote" "$work/ca.crt" "$work/keys"
}

coturn_logs() { # coturn logs only to the "sodium-remote" journal namespace, simple-log, never verbose
  "$HERE/deploy.sh" run '
    since=@$(systemctl show -P ActiveEnterTimestamp --timestamp=unix sodium-remote-coturn | tr -d @)
    ns=$(systemctl show -P LogNamespace sodium-remote-coturn)
    in_ns=$(journalctl --namespace=sodium-remote _COMM=turnserver --since "$since" -o cat | wc -l)
    in_sys=$(journalctl _COMM=turnserver --since "$since" -o cat | wc -l)
    conf=$(grep -vE "^(#|$)" /etc/systemd/journald@sodium-remote.conf | tr "\n" " ")
    simple=$(grep -c "^simple-log$" /etc/sodium-remote-server/turnserver.conf)
    verbose=$(grep -ciE "^verbose" /etc/sodium-remote-server/turnserver.conf)
    echo "LogNamespace=$ns; coturn lines since start: namespace remote $in_ns, system journal $in_sys"
    echo "journald@sodium-remote.conf: $conf"
    echo "simple-log lines $simple, verbose lines $verbose"
    [ "$ns" = sodium-remote ] && [ "$in_ns" -gt 0 ] && [ "$in_sys" = 0 ] && [ "$simple" = 1 ] && [ "$verbose" = 0 ] \
      && grep -q "^MaxRetentionSec=7day$" /etc/systemd/journald@sodium-remote.conf \
      && grep -q "^SystemMaxUse=200M$" /etc/systemd/journald@sodium-remote.conf \
      && ! grep -qE "^Forward[A-Za-z]*=yes" /etc/systemd/journald@sodium-remote.conf'
}

turn_check() { # transport, extra flags: allocate with the issued credentials, relay both ways
  local extra=(--stun-port "$TURN_PORT") p
  [ "$1" = tls ] && extra+=(--port "$TURNS_PORT") || extra+=(--port "$TURN_PORT")
  [ -z "${SODIUM_REMOTE_PRIVATE_IP:-}" ] || extra+=(--own-ip "$SODIUM_REMOTE_PRIVATE_IP")
  for p in ${SODIUM_REMOTE_DENY_PEERS:-}; do extra+=(--deny-peer "$p"); done
  python3 -I "$HERE/turn-check.py" "$NAME" "$@" "$work/turn.json" --ca "$work/ca.crt" "${extra[@]}"
}

relay_to_relay() { # two allocations on this server relaying to each other (turnutils_uclient -y)
  local user cred o
  user=$(jq -r .username "$work/turn.json")
  cred=$(jq -r .credential "$work/turn.json")
  o=$(turnutils_uclient -p "$TURN_PORT" -u "$user" -w "$cred" -y -c -n 20 -l 120 "$ip" 2>&1)
  echo "${o//$cred/<credential>}" | grep -E 'Total lost|round trip|error'
  grep -q 'Total lost packets 0 (' <<<"$o"
}

guard_packets() { # packets the nft guard has dropped from the turnserver user to private IPv4 space
  "$HERE/deploy.sh" run 'nft list table inet sodium_remote_guard' \
    | awk '/ip daddr \{/ && /reject/ { for (i = 1; i < NF; i++) if ($i == "packets") print $(i + 1) }'
}

egress_guard() { # the probe packet coturn let through to its own private address was dropped
  local now
  now=$(guard_packets)
  echo "nft guard drops (turnserver -> private IPv4): before the TURN checks $guard_before, after $now"
  [ "$now" -gt "$guard_before" ]
}

check tls-rendezvous tls_chain "$NAME" "$RDV_PORT"
check tls-turns tls_chain "$NAME" "$TURNS_PORT"
check healthz healthz
check stun-udp stun_udp
check rendezvous-api rendezvous
check rendezvous-sleep-notice sleep_notice
guard_before=$(guard_packets)
check turn-udp turn_check udp
check turn-tcp turn_check tcp --no-deny
check turn-tls turn_check tls --no-deny
check turn-relay-to-relay relay_to_relay
check turn-refuses-bad-password turn_check udp --bad-password
check relay-egress-guard egress_guard
check coturn-log-namespace coturn_logs
echo "passed $pass, failed $fail" | tee "$OUT/summary.txt"
[ "$fail" = 0 ]
