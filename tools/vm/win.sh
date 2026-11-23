#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# Control the Windows 11 test VM for Sodium Remote. See README.md; settings in config.sh / local.env.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091
source "$HERE/config.sh"
VMDIR="$REMOTE_VM_DIR"
DISK="$VMDIR/disk.qcow2"
VARS="$VMDIR/OVMF_VARS.fd"
QMP="$VMDIR/qmp.sock"
PIDFILE="$VMDIR/qemu.pid"
TPMDIR="$VMDIR/tpm"
OVMF_CODE=/usr/share/edk2/x64/OVMF_CODE.secboot.4m.fd

ssh_opts=(-i "$SSH_KEY" -o IdentitiesOnly=yes -o StrictHostKeyChecking=no
          -o UserKnownHostsFile="$VMDIR/known_hosts" -o LogLevel=ERROR
          -o ConnectTimeout=10 -o BatchMode=yes)

die() { echo "win.sh: $*" >&2; exit 1; }

running() { [[ -f $PIDFILE ]] && kill -0 "$(cat "$PIDFILE")" 2>/dev/null; }

qmp() { # qmp '{"execute":"..."}' ...  -> prints each response line
  python3 -I - "$QMP" "$@" <<'PY'
import json, socket, sys
s = socket.socket(socket.AF_UNIX); s.settimeout(30); s.connect(sys.argv[1])
f = s.makefile("rw")
f.readline()
def call(c):
    f.write(c + "\n"); f.flush()
    while True:
        r = json.loads(f.readline())
        if "event" in r: continue
        return r
call('{"execute":"qmp_capabilities"}')
for c in sys.argv[2:]:
    print(json.dumps(call(c)))
PY
}

cmd_net_up() {
  if ! ip link show "$BR" >/dev/null 2>&1; then
    sudo -n ip link add "$BR" type bridge
    sudo -n ip addr add "$BR_IP/24" dev "$BR"
  fi
  if ! ip link show "$TAP" >/dev/null 2>&1; then
    sudo -n ip tuntap add dev "$TAP" mode tap user "$(id -un)"
    sudo -n ip link set "$TAP" master "$BR"
  fi
  sudo -n ip link set "$BR" up
  sudo -n ip link set "$TAP" up
  # The bridge is host-only: no forwarding, no NAT, nothing enslaved but the tap.
  ip -br addr show "$BR"
}

cmd_net_down() {
  running && die "VM is running; stop it first"
  if ip link show "$TAP" >/dev/null 2>&1; then sudo -n ip link del "$TAP"; fi
  if ip link show "$BR" >/dev/null 2>&1; then sudo -n ip link del "$BR"; fi
  echo "bridge and tap removed"
}

start_tpm() {
  mkdir -p "$TPMDIR"
  if [[ ! -f $TPMDIR/.setup-done ]]; then
    swtpm_setup --tpm2 --tpmstate "$TPMDIR" --createek --create-ek-cert --create-platform-cert \
      --lock-nvram --not-overwrite >"$TPMDIR/setup.log" 2>&1 || true
    touch "$TPMDIR/.setup-done"
  fi
  if ! { [[ -f $TPMDIR/swtpm.pid ]] && kill -0 "$(cat "$TPMDIR/swtpm.pid")" 2>/dev/null; }; then
    rm -f "$TPMDIR/swtpm.sock"
    swtpm socket --tpm2 --tpmstate dir="$TPMDIR" --ctrl type=unixio,path="$TPMDIR/swtpm.sock" \
      --pid file="$TPMDIR/swtpm.pid" --log file="$TPMDIR/swtpm.log",level=1 --terminate --daemon
  fi
}

# start [--install WIN_ISO UNATTEND_ISO]   headless; VNC on 127.0.0.1:$((5900+VNC_DISPLAY))
cmd_start() {
  if running; then echo "already running (pid $(cat "$PIDFILE"))"; return 0; fi
  [[ -f $DISK ]] || die "no disk at $DISK (run create.sh first)"
  ip link show "$TAP" >/dev/null 2>&1 || cmd_net_up >/dev/null
  start_tpm
  local media=()
  if [[ ${1:-} == --install ]]; then
    media=(-drive "file=$2,if=none,id=cd0,media=cdrom,readonly=on" -device ide-cd,drive=cd0,bus=ahci.1,bootindex=2
           -drive "file=$3,if=none,id=cd1,media=cdrom,readonly=on" -device ide-cd,drive=cd1,bus=ahci.2
           -drive "file=$VMDIR/virtio-win.iso,if=none,id=cd2,media=cdrom,readonly=on" -device ide-cd,drive=cd2,bus=ahci.3)
  fi
  rm -f "$QMP"
  qemu-system-x86_64 -name "$REMOTE_VM_NAME" \
    -machine q35,accel=kvm,smm=on -cpu Skylake-Client-v4,hv_relaxed,hv_vapic,hv_spinlocks=0x1fff,hv_time \
    -smp 6 -m 8192 -rtc base=localtime \
    -global driver=cfi.pflash01,property=secure,value=on \
    -drive "if=pflash,format=raw,unit=0,readonly=on,file=$OVMF_CODE" \
    -drive "if=pflash,format=raw,unit=1,file=$VARS" \
    -chardev "socket,id=chrtpm,path=$TPMDIR/swtpm.sock" -tpmdev emulator,id=tpm0,chardev=chrtpm \
    -device tpm-tis,tpmdev=tpm0 \
    -device ich9-ahci,id=ahci \
    -drive "file=$DISK,if=none,id=hd0,format=qcow2,cache=writeback,discard=unmap" \
    -device ide-hd,drive=hd0,bus=ahci.0,bootindex=1 \
    "${media[@]}" \
    -device VGA,vgamem_mb=64,xres=1920,yres=1080 \
    -audiodev none,id=snd0 -device intel-hda -device hda-duplex,audiodev=snd0 \
    -device qemu-xhci -device usb-tablet \
    -netdev user,id=nat0 -device e1000e,netdev=nat0,mac=52:54:00:12:34:56 \
    -netdev "tap,id=br0,ifname=$TAP,script=no,downscript=no" -device e1000e,netdev=br0,mac=$REMOTE_VM_MAC \
    -display none -vnc "127.0.0.1:$VNC_DISPLAY" \
    -qmp "unix:$QMP,server=on,wait=off" -pidfile "$PIDFILE" -daemonize
  echo "started (pid $(cat "$PIDFILE")), VNC 127.0.0.1:$((5900+VNC_DISPLAY)), guest $GUEST_IP"
}

cmd_stop() {
  if ! running; then echo "not running"; return 0; fi
  local pid; pid=$(cat "$PIDFILE")
  qmp '{"execute":"system_powerdown"}' >/dev/null
  for _ in $(seq 1 240); do
    if ! kill -0 "$pid" 2>/dev/null; then echo "stopped"; return 0; fi
    sleep 1
  done
  die "guest did not shut down within 240 s (still running; 'win.sh qmp {\"execute\":\"quit\"}' is the last resort)"
}

cmd_status() {
  if running; then
    echo "vm: running (pid $(cat "$PIDFILE")), VNC 127.0.0.1:$((5900+VNC_DISPLAY))"
    qmp '{"execute":"query-status"}' | python3 -I -c 'import json,sys;print("qemu:",json.loads(sys.stdin.readline())["return"]["status"])'
    if ssh "${ssh_opts[@]}" "$GUEST_USER@$GUEST_IP" 'exit 0' 2>/dev/null; then echo "ssh: up ($GUEST_IP)"; else echo "ssh: down"; fi
  else
    echo "vm: stopped"
  fi
  ip -br addr show "$BR" 2>/dev/null || echo "bridge: down"
  if [[ -f $DISK ]]; then qemu-img snapshot -l "$DISK" 2>/dev/null | sed 's/^/snap: /' || true; fi
}

cmd_ssh() { exec ssh "${ssh_opts[@]}" "$GUEST_USER@$GUEST_IP" "$@"; }
cmd_scp_to() { [[ $# -eq 2 ]] || die "usage: scp-to <local> <remote>"; scp -r "${ssh_opts[@]}" "$1" "$GUEST_USER@$GUEST_IP:$2"; }
cmd_scp_from() { [[ $# -eq 2 ]] || die "usage: scp-from <remote> <local>"; scp -r "${ssh_opts[@]}" "$GUEST_USER@$GUEST_IP:$1" "$2"; }

cmd_screenshot() {
  [[ $# -eq 1 ]] || die "usage: screenshot <file.png>"
  running || die "VM is not running"
  local ppm="$VMDIR/screendump.ppm"
  qmp "{\"execute\":\"screendump\",\"arguments\":{\"filename\":\"$ppm\"}}" >/dev/null
  magick "$ppm" "$1"; rm -f "$ppm"
  echo "$1"
}

cmd_snapshot() { [[ $# -eq 1 ]] || die "usage: snapshot <name>"; if running; then die "stop the VM first"; fi; qemu-img snapshot -c "$1" "$DISK"; }
cmd_revert()   { [[ $# -eq 1 ]] || die "usage: revert <name>";   if running; then die "stop the VM first"; fi; qemu-img snapshot -a "$1" "$DISK"; }

sub="${1:-}"; shift || true
# One VM, several builders: take the lock before a test that uses the guest's
# desktop, a reboot, stop, snapshot or revert; release it when done. A lock
# older than 90 minutes is stale and may be taken over.
LOCK="$VMDIR/lock.d"
cmd_lock() {
  [[ $# -eq 1 ]] || die "usage: lock <owner>"
  if mkdir "$LOCK" 2>/dev/null; then echo "$1 $(date +%s)" > "$LOCK/owner"; echo "locked by $1"; return; fi
  read -r who since < "$LOCK/owner" 2>/dev/null || { who=?; since=0; }
  [[ $who == "$1" ]] && { echo "$1 $(date +%s)" > "$LOCK/owner"; echo "already yours (renewed)"; return; }
  if (( $(date +%s) - since > 5400 )); then echo "$1 $(date +%s)" > "$LOCK/owner"; echo "took over a stale lock from $who"; return; fi
  echo "busy: held by $who for $(( ($(date +%s) - since) / 60 )) min" >&2; exit 3
}
cmd_unlock() {
  [[ $# -eq 1 ]] || die "usage: unlock <owner>"
  read -r who _ < "$LOCK/owner" 2>/dev/null || { echo "not locked"; return; }
  [[ $who == "$1" ]] || die "held by $who, not $1"
  rm -rf "$LOCK"; echo "unlocked"
}
cmd_who() { if [[ -f $LOCK/owner ]]; then read -r who since < "$LOCK/owner"; echo "$who for $(( ($(date +%s) - since) / 60 )) min"; else echo "free"; fi; }

case "$sub" in
  net-up) cmd_net_up ;;
  net-down) cmd_net_down ;;
  start) cmd_start "$@" ;;
  stop) cmd_stop ;;
  status) cmd_status ;;
  ssh) cmd_ssh "$@" ;;
  scp-to) cmd_scp_to "$@" ;;
  scp-from) cmd_scp_from "$@" ;;
  screenshot) cmd_screenshot "$@" ;;
  snapshot) cmd_snapshot "$@" ;;
  revert) cmd_revert "$@" ;;
  qmp) qmp "$@" ;;
  lock) cmd_lock "$@" ;;
  unlock) cmd_unlock "$@" ;;
  who) cmd_who ;;
  *) echo "usage: win.sh net-up|net-down|start|stop|status|ssh [cmd]|scp-to L R|scp-from R L|screenshot F.png|snapshot N|revert N" >&2; exit 2 ;;
esac
