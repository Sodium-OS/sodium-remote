#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# Runs on the relay server as root (deploy.sh runs it through sudo when the SSH user
# isn't root); deploy.sh uploads it with the files beside it. Debian-family (apt).
# Idempotent: every step looks before it changes anything, and only services whose
# inputs changed are restarted. It never stops, restarts or edits anything that
# isn't Sodium Remote's own (sodium-remote-server, sodium-remote-coturn,
# /etc/sodium-remote-server, the sodium_remote_guard nft table, and the coturn
# package it installs).
#
# Env (deploy.sh passes these):
#   SERVER_NAME   required. The relay's public DNS name, e.g. relay.example.com.
#   RDV_PORT      rendezvous port (default 8443)
#   TURN_PORT / TURNS_PORT   STUN/TURN and TURN-over-TLS ports (3478 / 5349)
#   MIN_PORT / MAX_PORT      coturn relay port range (50000 / 50099)
#   ENROL         open | token | allowlist (default token)
#   TLS_MODE      provided (default): you put a CA-signed certificate (for example
#                 Let's Encrypt) at /etc/sodium-remote-server/tls/server.crt (full
#                 chain) and server.key; this script never creates one.
#                 private-ca: this script makes a private CA and signs a 1-year
#                 leaf (clients must pin ca.crt; see deploy.sh fetch-ca).
#   FORCE_LEAF    1 = issue a new private-CA leaf now (deploy.sh rotate-leaf)
#   USER_QUOTA (12), TOTAL_QUOTA (60), MAX_BPS (5000000)   coturn limits
#   PRIVATE_IP / PUBLIC_IP   override the detected addresses (the public one comes
#                 from DNS for SERVER_NAME, the private one from the default route)
#   REALM         coturn realm (default SERVER_NAME)
set -euo pipefail
umask 022
export LC_ALL=C

STAGE=$(cd "$(dirname "$0")" && pwd)
[ "$(id -u)" = 0 ] || { echo "server-setup.sh must run as root" >&2; exit 1; }
: "${SERVER_NAME:?SERVER_NAME is required (the relay DNS name)}"
RDV_PORT=${RDV_PORT:-8443}
TURN_PORT=${TURN_PORT:-3478}
TURNS_PORT=${TURNS_PORT:-5349}
MIN_PORT=${MIN_PORT:-50000}
MAX_PORT=${MAX_PORT:-50099}
ENROL=${ENROL:-token}
TLS_MODE=${TLS_MODE:-provided}
USER_QUOTA=${USER_QUOTA:-12}
TOTAL_QUOTA=${TOTAL_QUOTA:-60}
MAX_BPS=${MAX_BPS:-5000000}
REALM=${REALM:-$SERVER_NAME}
FORCE_LEAF=${FORCE_LEAF:-0}
ETC=/etc/sodium-remote-server
case $ENROL in open | token | allowlist) ;; *) echo "ENROL must be open, token or allowlist" >&2; exit 1 ;; esac
case $TLS_MODE in provided | private-ca) ;; *) echo "TLS_MODE must be provided or private-ca" >&2; exit 1 ;; esac
restart_rdv=0
restart_turn=0
reload_units=0

say() { printf '%s\n' "$*"; }
die() { printf 'ABORT: %s\n' "$*" >&2; exit 1; }

# install_if_changed SRC DST MODE: installs only when content or mode differ.
install_if_changed() {
  local src=$1 dst=$2 mode=$3
  if [ -f "$dst" ] && cmp -s "$src" "$dst" && [ "$(stat -c %a "$dst")" = "${mode#0}" ]; then
    say "= $dst"
    return 1
  fi
  install -m "$mode" -o root -g root "$src" "$dst"
  say "+ $dst"
  return 0
}

# --- 1. Preflight: our ports must be free or already ours. Nothing else is touched.
ours='^(sodium-remote-s|turnserver)$'
for spec in "t $RDV_PORT" "t $TURN_PORT" "u $TURN_PORT" "t $TURNS_PORT"; do
  set -- $spec
  owners=$(ss -H -ln"$1"p "sport = :$2" | grep -o 'users:(("[^"]*"' | sed 's/users:(("//; s/"$//' | sort -u || true)
  for o in $owners; do
    [[ $o =~ $ours ]] || die "port $2/$1 is used by $o; not touching it"
  done
done
say "= ports $RDV_PORT/tcp $TURN_PORT/tcp+udp $TURNS_PORT/tcp free or ours"

# --- 2. coturn from Debian, with the stock unit masked so it never runs the default config.
if [ "$(systemctl is-enabled coturn.service 2>/dev/null || true)" != masked ]; then
  systemctl mask coturn.service >/dev/null 2>&1
  say "+ masked the stock coturn.service"
fi
if ! dpkg-query -W -f='${Status}' coturn 2>/dev/null | grep -q 'install ok installed'; then
  export DEBIAN_FRONTEND=noninteractive
  apt_log=$(mktemp /var/tmp/sodium-remote-apt.XXXXXX.log)
  apt-get install -y --no-install-recommends coturn >"$apt_log" 2>&1 \
    || { apt-get update >>"$apt_log" 2>&1 \
         && apt-get install -y --no-install-recommends coturn >>"$apt_log" 2>&1; } \
    || die "installing coturn failed; see $apt_log"
  rm -f "$apt_log"
  say "+ installed coturn $(dpkg-query -W -f='${Version}' coturn)"
else
  say "= coturn $(dpkg-query -W -f='${Version}' coturn)"
fi
id turnserver >/dev/null || die "the coturn package did not create the turnserver user"

# --- 3. Secrets, generated here, root-only, never printed.
install -d -m 0755 -o root -g root "$ETC"
install -d -m 0700 -o root -g root "$ETC/tls"
if [ ! -s "$ETC/turn-secret" ]; then
  (umask 077; od -An -N32 -tx1 /dev/urandom | tr -d ' \n' > "$ETC/turn-secret")
  say "+ generated $ETC/turn-secret"
  restart_rdv=1; restart_turn=1
else
  say "= $ETC/turn-secret"
fi
chmod 0600 "$ETC/turn-secret"

# --- 4. TLS. Default (TLS_MODE=provided): a CA-signed certificate (Let's Encrypt or
# any other) that the operator places at $ETC/tls/server.crt (full chain) and
# server.key; it is only checked here, and the units re-read it when restarted.
# TLS_MODE=private-ca: a private CA made here (10 years, key root-only), which
# clients pin, signing 1-year leaves. A leaf is reissued 30 days before it expires,
# or now with FORCE_LEAF=1.
if [ "$TLS_MODE" = provided ]; then
  [ -s "$ETC/tls/server.crt" ] && [ -s "$ETC/tls/server.key" ] \
    || die "no certificate: put the full chain at $ETC/tls/server.crt and the key at $ETC/tls/server.key (Let's Encrypt or any CA), or set TLS_MODE=private-ca"
  chmod 0600 "$ETC/tls/server.key"
  openssl x509 -checkend 0 -noout -in "$ETC/tls/server.crt" >/dev/null || die "$ETC/tls/server.crt has expired"
  say "= TLS certificate ($(openssl x509 -noout -enddate -in "$ETC/tls/server.crt"))"
elif [ "$FORCE_LEAF" = 1 ] || [ ! -s "$ETC/tls/server.crt" ] \
   || ! openssl x509 -checkend $((30 * 86400)) -noout -in "$ETC/tls/server.crt" >/dev/null; then
  t=$(mktemp -d)
  (
    umask 077
    if [ ! -s "$ETC/tls/ca.key" ]; then
      openssl req -x509 -new -noenc -newkey ec -pkeyopt ec_paramgen_curve:P-256 -sha256 -days 3650 \
        -subj "/CN=Sodium Remote relay CA ($SERVER_NAME)" \
        -addext "basicConstraints=critical,CA:TRUE" -addext "keyUsage=critical,keyCertSign,cRLSign" \
        -keyout "$ETC/tls/ca.key" -out "$ETC/tls/ca.crt" 2>/dev/null
    fi
    openssl req -new -noenc -newkey ec -pkeyopt ec_paramgen_curve:P-256 -subj "/CN=$SERVER_NAME" \
      -keyout "$t/server.key" -out "$t/server.csr" 2>/dev/null
    printf 'subjectAltName=DNS:%s\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\n' \
      "$SERVER_NAME" > "$t/ext"
    openssl x509 -req -sha256 -days 365 -in "$t/server.csr" -CA "$ETC/tls/ca.crt" -CAkey "$ETC/tls/ca.key" \
      -set_serial "0x$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')" -extfile "$t/ext" -out "$t/leaf.crt" 2>/dev/null
    cat "$t/leaf.crt" "$ETC/tls/ca.crt" > "$t/server.crt"
    install -m 0600 "$t/server.key" "$ETC/tls/server.key"
    install -m 0644 "$t/server.crt" "$ETC/tls/server.crt"
  )
  rm -rf "$t"
  chmod 0600 "$ETC/tls/ca.key"
  chmod 0644 "$ETC/tls/ca.crt"
  say "+ issued the TLS certificate for $SERVER_NAME (private CA)"
  restart_rdv=1; restart_turn=1
else
  say "= TLS certificate ($(openssl x509 -noout -enddate -in "$ETC/tls/server.crt"))"
fi

# --- 5. The host allowlist, only for ENROL=allowlist (public device IDs; edited with
# deploy.sh allow-host). In token mode hosts enrol with deploy.sh token-new.
hosts_allow=""
if [ "$ENROL" = allowlist ]; then
  if [ ! -e "$ETC/hosts.allow" ]; then
    printf '# Host device IDs allowed to register as Sodium Remote hosts, one per line.\n# Edit with deploy.sh allow-host/disallow-host (reloads sodium-remote-server).\n' > "$ETC/hosts.allow"
    chmod 0644 "$ETC/hosts.allow"
    say "+ $ETC/hosts.allow"
  fi
  hosts_allow="--hosts-allow $ETC/hosts.allow"
fi

# --- 6. Render the coturn config and the egress guard. Bash builtins only, so the
# secret never appears on a command line.
private_ip=${PRIVATE_IP:-}
public_ip=${PUBLIC_IP:-}
[ -n "$private_ip" ] || private_ip=$(ip -4 -o route get 1.1.1.1 | sed -n 's/.* src \([0-9.]*\).*/\1/p')
[ -n "$public_ip" ] || public_ip=$(getent ahostsv4 "$SERVER_NAME" | awk 'NR == 1 { print $1 }')
# A host that has its public address itself needs no external-ip mapping.
if [ "$public_ip" = "$private_ip" ]; then external_ip="# external-ip: not needed, the server has its public address"; else external_ip="external-ip=$public_ip/$private_ip"; fi
[ -n "$private_ip" ] && [ -n "$public_ip" ] || die "could not work out the private ($private_ip) or public ($public_ip) address"
deny_lines=""
nft_resolvers=""
while read -r ns; do
  deny_lines+="denied-peer-ip=$ns"$'\n'
  nft_resolvers+=", $ns"
done < <(awk '/^nameserver/ { print $2 }' /etc/resolv.conf | grep -E '^[0-9]+(\.[0-9]+){3}$' || true)

secret=$(<"$ETC/turn-secret")
conf=$(<"$STAGE/turnserver.conf.in")
conf=${conf//@PRIVATE_IP@/$private_ip}
conf=${conf//@EXTERNAL_IP@/$external_ip}
conf=${conf//@PUBLIC_IP@/$public_ip}
conf=${conf//@SERVER_NAME@/$SERVER_NAME}
conf=${conf//@REALM@/$REALM}
conf=${conf//@TURNS_PORT@/$TURNS_PORT}
conf=${conf//@TURN_PORT@/$TURN_PORT}
conf=${conf//@MIN_PORT@/$MIN_PORT}
conf=${conf//@MAX_PORT@/$MAX_PORT}
conf=${conf//@USER_QUOTA@/$USER_QUOTA}
conf=${conf//@TOTAL_QUOTA@/$TOTAL_QUOTA}
conf=${conf//@MAX_BPS@/$MAX_BPS}
conf=${conf//@DENY_RESOLVERS@/${deny_lines%$'\n'}}
conf=${conf//@SECRET@/$secret}
unset secret
(umask 077; printf '%s\n' "$conf" > "$STAGE/turnserver.conf")
unset conf
if install_if_changed "$STAGE/turnserver.conf" "$ETC/turnserver.conf" 0600; then restart_turn=1; fi
rm -f "$STAGE/turnserver.conf"

guard=$(<"$STAGE/relay-guard.nft.in")
guard=${guard//@PRIVATE_IP@/$private_ip}
guard=${guard//@MIN_PORT@/$MIN_PORT}
guard=${guard//@MAX_PORT@/$MAX_PORT}
printf '%s\n' "${guard//@RESOLVERS@/$nft_resolvers}" > "$STAGE/relay-guard.nft"
if install_if_changed "$STAGE/relay-guard.nft" "$ETC/relay-guard.nft" 0644; then
  nft -f "$ETC/relay-guard.nft"
  say "+ loaded nft table inet sodium_remote_guard"
fi

# --- 7. coturn's journal namespace, then the units and the binary.
if install_if_changed "$STAGE/journald@sodium-remote.conf" /etc/systemd/journald@sodium-remote.conf 0644; then
  # Only our namespace's journald reads this file; restart it if it's already running.
  systemctl try-restart systemd-journald@sodium-remote.service
fi
unit=$(<"$STAGE/sodium-remote-server.service.in")
unit=${unit//@SERVER_NAME@/$SERVER_NAME}
unit=${unit//@RDV_PORT@/$RDV_PORT}
unit=${unit//@TURN_PORT@/$TURN_PORT}
unit=${unit//@TURNS_PORT@/$TURNS_PORT}
unit=${unit//@ENROL@/$ENROL}
printf '%s\n' "${unit//@HOSTS_ALLOW@/$hosts_allow}" > "$STAGE/sodium-remote-server.service"
if install_if_changed "$STAGE/sodium-remote-server.service" /etc/systemd/system/sodium-remote-server.service 0644; then
  reload_units=1; restart_rdv=1
fi
if install_if_changed "$STAGE/sodium-remote-coturn.service" /etc/systemd/system/sodium-remote-coturn.service 0644; then
  reload_units=1; restart_turn=1
fi
if install_if_changed "$STAGE/sodium-remote-server" /usr/local/bin/sodium-remote-server 0755; then restart_rdv=1; fi
[ "$reload_units" = 1 ] && systemctl daemon-reload && say "+ systemctl daemon-reload"

# --- 8. Enable and (re)start only what changed.
systemctl enable sodium-remote-coturn.service sodium-remote-server.service >/dev/null 2>&1
for pair in "sodium-remote-coturn $restart_turn" "sodium-remote-server $restart_rdv"; do
  set -- $pair
  if [ "$2" = 1 ]; then
    systemctl restart "$1.service"
    say "+ restarted $1"
  elif ! systemctl is-active --quiet "$1.service"; then
    systemctl start "$1.service"
    say "+ started $1"
  else
    say "= $1 running"
  fi
done

# --- 9. The host firewall: report, never rewrite. Sodium Remote needs these inbound:
# TCP $RDV_PORT, UDP+TCP $TURN_PORT, TCP $TURNS_PORT, UDP $MIN_PORT-$MAX_PORT.
ufw_status=$(ufw status 2>/dev/null || true)
input_chain=$(nft list chain ip filter INPUT 2>/dev/null || true)
if [[ $ufw_status == *"Status: active"* ]]; then
  say "! ufw is active: open TCP $RDV_PORT, UDP+TCP $TURN_PORT, TCP $TURNS_PORT and UDP $MIN_PORT-$MAX_PORT"
elif [[ $input_chain == *"policy accept"* ]]; then
  say "= host firewall: INPUT policy accept, nothing to open"
else
  say "! host firewall: check that TCP $RDV_PORT, UDP+TCP $TURN_PORT, TCP $TURNS_PORT and UDP $MIN_PORT-$MAX_PORT are open"
fi

# --- 10. Wait for the listeners.
for _ in $(seq 1 20); do
  if ss -H -lnt "sport = :$RDV_PORT" | grep -q . && ss -H -lnu "sport = :$TURN_PORT" | grep -q . \
     && ss -H -lnt "sport = :$TURNS_PORT" | grep -q .; then
    break
  fi
  sleep 0.5
done
say "--- listening"
ss -H -lntup | grep -E ":($RDV_PORT|$TURN_PORT|$TURNS_PORT) " | awk '{ print $1, $5, $7 }'
say "--- services"
for u in sodium-remote-server sodium-remote-coturn; do say "$u: $(systemctl is-active $u.service) / $(systemctl is-enabled $u.service)"; done
