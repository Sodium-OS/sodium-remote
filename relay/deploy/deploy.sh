#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# Deploys the Sodium Remote relay (sodium-remote-server + coturn) to a Debian-family
# server you run, over SSH. Idempotent: run it again any time; only what changed is
# replaced and restarted. Nothing here has a default server: you name yours.
#
#   SODIUM_REMOTE_SSH=admin@relay.example.com SODIUM_REMOTE_SERVER_NAME=relay.example.com ./deploy.sh
#
#   deploy.sh [deploy]              build here (release, x86_64) and deploy
#   deploy.sh rotate-leaf           deploy, issuing a new 1-year leaf from the private CA now
#                                   (only with SODIUM_REMOTE_TLS_MODE=private-ca)
#   deploy.sh status                services, listeners, recent log lines
#   deploy.sh token-new [--uses N] [--expires 30m|24h|7d] [--account LABEL]
#                                   mint a host enrolment token (default enrolment mode);
#                                   the host pastes it into server.toml as token = "..."
#   deploy.sh token-list            list the tokens
#   deploy.sh token-revoke ID       revoke one
#   deploy.sh host-id               this machine's host device ID (from its host key), then its
#                                   short code XXXX-XXXX on the next line, as `remoted id` prints both
#   deploy.sh allow-host [id]       (SODIUM_REMOTE_ENROL=allowlist) enrol a host (default: this
#                                   machine's); reloads the server
#   deploy.sh disallow-host [id]    remove it (default: this machine's); its connection drops
#   deploy.sh run 'COMMAND'         run a shell command on the server as root (used by verify.sh)
#   deploy.sh fetch-ca <file>       (private-ca) copy the CA certificate (public) here, to bundle
#
# TLS: the default is a certificate from a public CA (Let's Encrypt or any other): put the
# full chain at /etc/sodium-remote-server/tls/server.crt and the key at server.key on the
# server (the setup stops with a message until they exist; e.g. certbot, then copy or
# link them). For operators without Web PKI, SODIUM_REMOTE_TLS_MODE=private-ca makes a
# private CA on the server that clients pin (fetch-ca); rotate-leaf renews its leaf.
#
# Environment:
#   SODIUM_REMOTE_SSH          required (except host-id): the SSH target, e.g. admin@relay.example.com.
#                              A non-root user needs passwordless sudo; sudo is used for everything
#                              that needs root.
#   SODIUM_REMOTE_SERVER_NAME  required for deploy/rotate-leaf: the relay's public DNS name
#   SODIUM_REMOTE_SSH_KEY      optional: an SSH identity file (otherwise ssh's own defaults/agent)
#   SODIUM_REMOTE_SSH_PORT     optional: the SSH port
#   SODIUM_REMOTE_PORT         rendezvous port (8443)
#   SODIUM_REMOTE_TURN_PORT    STUN/TURN port, UDP+TCP (3478)
#   SODIUM_REMOTE_TURNS_PORT   TURN over TLS port (5349)
#   SODIUM_REMOTE_MIN_PORT / SODIUM_REMOTE_MAX_PORT   coturn relay port range (50000 / 50099)
#   SODIUM_REMOTE_ENROL        open | token | allowlist (token)
#   SODIUM_REMOTE_TLS_MODE     provided | private-ca (provided)
#   SODIUM_REMOTE_USER_QUOTA   TURN allocations per username (12)
#   SODIUM_REMOTE_TOTAL_QUOTA  TURN allocations in total (60)
#   SODIUM_REMOTE_MAX_BPS      TURN bytes per second per allocation (5000000 = 5 MB/s)
#   SODIUM_REMOTE_REALM        coturn realm (the server name)
#   SODIUM_REMOTE_PRIVATE_IP / SODIUM_REMOTE_PUBLIC_IP   override the addresses the setup detects
#   SODIUM_REMOTE_HOST_KEY     the host's Ed25519 device key, PKCS#8 PEM, for host-id/allow-host
#                              (~/.local/share/sodium-remote/host/device.key); only the public
#                              device ID derived from it leaves this machine.
set -euo pipefail

SSH_TARGET=${SODIUM_REMOTE_SSH:-}
KEY=${SODIUM_REMOTE_SSH_KEY:-}
SSH_PORT=${SODIUM_REMOTE_SSH_PORT:-}
SERVER_NAME=${SODIUM_REMOTE_SERVER_NAME:-}
RDV_PORT=${SODIUM_REMOTE_PORT:-8443}
TURN_PORT=${SODIUM_REMOTE_TURN_PORT:-3478}
TURNS_PORT=${SODIUM_REMOTE_TURNS_PORT:-5349}
MIN_PORT=${SODIUM_REMOTE_MIN_PORT:-50000}
MAX_PORT=${SODIUM_REMOTE_MAX_PORT:-50099}
ENROL=${SODIUM_REMOTE_ENROL:-token}
TLS_MODE=${SODIUM_REMOTE_TLS_MODE:-provided}
HERE=$(cd "$(dirname "$0")" && pwd)
WORKSPACE=$(cd "$HERE/../.." && pwd)
CONF_DIR=/etc/sodium-remote-server
STATE_DIR=/var/lib/sodium-remote-server
BIN=/usr/local/bin/sodium-remote-server
HOST_KEY=${SODIUM_REMOTE_HOST_KEY:-$HOME/.local/share/sodium-remote/host/device.key}
FORCE_LEAF=0

usage() {
  sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'
  exit 2
}

need_ssh() {
  [ -n "$SSH_TARGET" ] || {
    echo "SODIUM_REMOTE_SSH is not set: name the server's SSH target, e.g. SODIUM_REMOTE_SSH=admin@relay.example.com" >&2
    exit 2
  }
}
need_name() {
  [ -n "$SERVER_NAME" ] || {
    echo "SODIUM_REMOTE_SERVER_NAME is not set: name the relay, e.g. SODIUM_REMOTE_SERVER_NAME=relay.example.com" >&2
    exit 2
  }
}

ssh_() {
  local opts=(-o BatchMode=yes)
  [ -z "$KEY" ] || opts+=(-i "$KEY" -o IdentitiesOnly=yes)
  [ -z "$SSH_PORT" ] || opts+=(-p "$SSH_PORT")
  ssh "${opts[@]}" "$SSH_TARGET" "$@"
}

# rsh CMD: run a shell command string on the server as root (through sudo -n unless the
# SSH user is root).
rsh() {
  if [ "${SSH_TARGET%%@*}" = root ]; then ssh_ "$1"; else ssh_ "sudo -n bash -c $(printf '%q' "$1")"; fi
}

valid_id() { [[ $1 =~ ^[A-Za-z0-9_-]{43}$ ]] || { echo "not a device ID: $1" >&2; exit 2; }; }

# This machine's host device ID: base64url (no padding) of the raw 32-byte Ed25519
# public key, derived from the private key file without sending it anywhere.
local_host_id() {
  [ -r "$HOST_KEY" ] || {
    echo "no host key at $HOST_KEY (remoted writes it: an Ed25519 private key, PKCS#8 PEM)" >&2
    exit 1
  }
  openssl pkey -in "$HOST_KEY" -noout -text 2>/dev/null | head -1 | grep -q '^ED25519 Private-Key' \
    || { echo "$HOST_KEY is not an Ed25519 private key in PEM" >&2; exit 1; }
  local id
  id=$(openssl pkey -in "$HOST_KEY" -pubout -outform DER | tail -c 32 | base64 -w0 | tr '+/' '-_' | tr -d '=')
  valid_id "$id"
  echo "$id"
}

# The short code people compare by eye (DeviceId::code() in remote-core): the first
# 40 bits of SHA-256 over the 32 raw public-key bytes, as 8 Crockford base32 characters, XXXX-XXXX.
code_of_id() {
  local hex v i c="" alphabet=0123456789ABCDEFGHJKMNPQRSTVWXYZ
  hex=$(printf '%s=' "$1" | tr '_-' '/+' | base64 -d | openssl dgst -sha256 -binary | head -c 5 | od -An -tx1 | tr -d ' \n')
  v=$((16#$hex))
  for i in 0 1 2 3 4 5 6 7; do c+=${alphabet:$(((v >> (35 - 5 * i)) & 31)):1}; done
  echo "${c:0:4}-${c:4:4}"
}

cmd_host_id() {
  local id
  id=$(local_host_id) || exit 1
  echo "$id"
  code_of_id "$id"
}

cmd_deploy() {
  need_name
  echo "--- build (release) here"
  cargo build --release -p remote-rendezvous --manifest-path "$WORKSPACE/Cargo.toml"
  local bin="$WORKSPACE/target/release/sodium-remote-server"
  local need have
  need=$(objdump -T "$bin" | grep -o 'GLIBC_[0-9.]*' | sed 's/GLIBC_//' | sort -uV | tail -1)
  have=$(ssh_ "ldd --version | head -1 | awk '{ print \$NF }'")
  [ "$(printf '%s\n%s\n' "$need" "$have" | sort -V | tail -1)" = "$have" ] \
    || { echo "the binary needs glibc $need; the server has $have" >&2; exit 1; }
  [ "$(ssh_ dpkg --print-architecture)" = amd64 ] || { echo "the server is not amd64" >&2; exit 1; }
  echo "= binary needs glibc $need, server has $have"

  echo "--- upload"
  local stage stage_remote
  stage=$(mktemp -d)
  trap 'rm -rf "$stage"' RETURN
  stage_remote=$(ssh_ 'echo "$HOME"')/.sodium-remote-stage
  cp "$bin" "$HERE/server-setup.sh" "$HERE/turnserver.conf.in" "$HERE/relay-guard.nft.in" \
     "$HERE/sodium-remote-server.service.in" "$HERE/sodium-remote-coturn.service" \
     "$HERE/journald@sodium-remote.conf" "$stage/"
  tar -C "$stage" -czf - . | ssh_ "rm -rf $stage_remote && install -d -m 0700 $stage_remote && tar -C $stage_remote -xzf -"

  echo "--- set up on $SSH_TARGET"
  local env_args="" v
  for v in SERVER_NAME RDV_PORT TURN_PORT TURNS_PORT MIN_PORT MAX_PORT ENROL TLS_MODE FORCE_LEAF; do
    env_args+=" $v=$(printf '%q' "${!v}")"
  done
  for pair in USER_QUOTA:SODIUM_REMOTE_USER_QUOTA TOTAL_QUOTA:SODIUM_REMOTE_TOTAL_QUOTA MAX_BPS:SODIUM_REMOTE_MAX_BPS \
              REALM:SODIUM_REMOTE_REALM PRIVATE_IP:SODIUM_REMOTE_PRIVATE_IP PUBLIC_IP:SODIUM_REMOTE_PUBLIC_IP; do
    v=${pair#*:}
    [ -z "${!v:-}" ] || env_args+=" ${pair%%:*}=$(printf '%q' "${!v}")"
  done
  rsh "env$env_args bash $stage_remote/server-setup.sh; rc=\$?; rm -rf $stage_remote; exit \$rc"
}

cmd_status() {
  rsh "for u in sodium-remote-server sodium-remote-coturn; do echo \"\$u: \$(systemctl is-active \$u) / \$(systemctl is-enabled \$u)\"; done
       ss -H -lntup | grep -E ':($RDV_PORT|$TURN_PORT|$TURNS_PORT) ' | awk '{ print \$1, \$5, \$7 }'
       echo '--- enrolment'; systemctl cat sodium-remote-server | grep -oE -- '--enrol [a-z]+'
       echo '--- TLS certificate'; openssl x509 -noout -enddate -in $CONF_DIR/tls/server.crt
       echo '--- server log'; journalctl -u sodium-remote-server -n 10 --no-pager -o cat
       echo '--- coturn log (journal namespace sodium-remote)'; journalctl --namespace=sodium-remote -u sodium-remote-coturn -n 10 --no-pager -o cat"
}

# The tokens live in the server's state dir; the server runs as a systemd dynamic user
# of the same name, so the CLI runs as that user too (a transient unit with the same
# user name and state directory), keeping the files readable by the service.
server_cli() {
  local args=""
  for a in "$@"; do args+=" $(printf '%q' "$a")"; done
  rsh "systemd-run --quiet --pipe --wait --collect -p DynamicUser=yes -p User=sodium-remote-server -p StateDirectory=sodium-remote-server $BIN$args --state-dir $STATE_DIR"
}

cmd_allow_host() {
  valid_id "$1"
  [ "$ENROL" = allowlist ] || echo "note: the server enrols by $ENROL; allow-host only matters with SODIUM_REMOTE_ENROL=allowlist" >&2
  rsh "grep -qxF '$1' $CONF_DIR/hosts.allow || echo '$1' >> $CONF_DIR/hosts.allow
       systemctl reload sodium-remote-server && echo 'allowed $1'"
}

cmd_disallow_host() {
  valid_id "$1"
  rsh "grep -vxF '$1' $CONF_DIR/hosts.allow > $CONF_DIR/hosts.allow.new || true
       chmod 0644 $CONF_DIR/hosts.allow.new
       mv $CONF_DIR/hosts.allow.new $CONF_DIR/hosts.allow
       systemctl reload sodium-remote-server && echo 'disallowed $1'"
}

cmd=${1:-deploy}
[ $# -eq 0 ] || shift
case "$cmd" in
  host-id) cmd_host_id; exit ;;
  deploy | rotate-leaf | status | token-new | token-list | token-revoke | allow-host | disallow-host | fetch-ca | run) need_ssh ;;
  -h | --help | help) usage ;;
  *) usage ;;
esac
case "$cmd" in
  deploy) cmd_deploy ;;
  rotate-leaf)
    [ "$TLS_MODE" = private-ca ] || { echo "rotate-leaf needs SODIUM_REMOTE_TLS_MODE=private-ca" >&2; exit 2; }
    FORCE_LEAF=1; cmd_deploy ;;
  status) cmd_status ;;
  token-new) server_cli token new "$@" ;;
  token-list) server_cli token list "$@" ;;
  token-revoke) server_cli token revoke "${1:?token ID}" ;;
  allow-host) cmd_allow_host "${1:-$(local_host_id)}" ;;
  disallow-host) cmd_disallow_host "${1:-$(local_host_id)}" ;;
  run) rsh "${1:?command}" ;;
  fetch-ca) rsh "cat $CONF_DIR/tls/ca.crt" > "${1:?output file}" && echo "wrote $1" ;;
esac
