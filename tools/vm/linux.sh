#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# Control the Arch GNOME/KDE test VM for Sodium Remote's Linux host. See README.md.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=config.sh
source "$HERE/config.sh"
VMDIR=$REMOTE_LINUX_VM_DIR
DISK="$VMDIR/arch-desktops.qcow2"
BOOT="$VMDIR/boot"
QMP="$VMDIR/qmp.sock"
PIDFILE="$VMDIR/qemu.pid"
BR=$REMOTE_VM_BRIDGE
BR_IP=$REMOTE_VM_NET.1
TAP=$REMOTE_LINUX_VM_TAP
GUEST_IP=$LX_GUEST_IP
VNC_DISPLAY=$REMOTE_LINUX_VM_VNC_DISPLAY
SSH_KEY=$REMOTE_LINUX_VM_SSH_KEY
GUEST_USER=$REMOTE_LINUX_VM_USER
DISK_SIZE=24G
RES_X=1920; RES_Y=1080

ssh_opts=(-i "$SSH_KEY" -o IdentitiesOnly=yes -o StrictHostKeyChecking=no
          -o UserKnownHostsFile="$VMDIR/known_hosts" -o LogLevel=ERROR
          -o ConnectTimeout=10 -o BatchMode=yes)

die() { echo "linux.sh: $*" >&2; exit 1; }

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
  # Host-only: no forwarding, no NAT, nothing enslaved but taps.
  ip -br addr show "$BR"
}

cmd_net_down() { # removes only our tap; the bridge goes too only if nothing else is on it
  running && die "VM is running; stop it first"
  if ip link show "$TAP" >/dev/null 2>&1; then sudo -n ip link del "$TAP"; fi
  if ip link show "$BR" >/dev/null 2>&1 && [[ -z $(ls "/sys/class/net/$BR/brif" 2>/dev/null) ]]; then
    sudo -n ip link del "$BR"; echo "tap and (now empty) bridge removed"
  else
    echo "tap removed; $BR kept (other ports on it)"
  fi
}

# create: build $REMOTE_LINUX_VM_DIR/arch-desktops.qcow2 with pacstrap (needs sudo -n, ~15 GB of host disk while building)
cmd_create() {
  [[ -f $DISK ]] && die "$DISK exists; delete it first to rebuild"
  mkdir -p "$BOOT" "$VMDIR/evidence"
  [[ -f $SSH_KEY ]] || ssh-keygen -q -t ed25519 -N '' -C remote-test-linux -f "$SSH_KEY"
  raw="$VMDIR/build.raw" mnt="$VMDIR/mnt" loop=""
  cleanup() {
    set +e
    sudo -n gpgconf --homedir "$mnt/etc/pacman.d/gnupg" --kill all 2>/dev/null   # pacstrap -K leaves a gpg-agent holding the mount
    sudo -n pkill -f "gpg-agent.*$mnt" 2>/dev/null
    sleep 1
    mountpoint -q "$mnt" && sudo -n umount -R "$mnt"
    [[ -n $loop ]] && sudo -n losetup -d "$loop"
  }
  trap cleanup EXIT
  rm -f "$raw"; truncate -s "$DISK_SIZE" "$raw"
  loop=$(sudo -n losetup -f --show "$raw")
  sudo -n mkfs.ext4 -q -L sr-root -m 0 "$loop"
  mkdir -p "$mnt"; sudo -n mount "$loop" "$mnt"
  # host pacman.conf + NoExtract (docs, man pages) so the guest stays lean; no -c: the host cache is untouched
  local conf="$VMDIR/pacman-build.conf"
  { cat /etc/pacman.conf; echo; } | sed '/^\[options\]/a NoExtract = usr/share/doc/* usr/share/man/* usr/share/info/* usr/share/gtk-doc/*' > "$conf"
  local pkgs=(base linux mkinitcpio openssh sudo mesa
    pipewire pipewire-pulse pipewire-alsa wireplumber alsa-utils libei ffmpeg openh264 opus libxkbcommon
    wl-clipboard python python-gobject gtk4 noto-fonts noto-fonts-cjk xorg-xwayland xdg-desktop-portal
    gnome-shell gnome-session gnome-settings-daemon gdm xdg-desktop-portal-gnome ptyxis
    plasma-desktop plasma-workspace kwin sddm xdg-desktop-portal-kde konsole)
  sudo -n pacstrap -K -C "$conf" "$mnt" "${pkgs[@]}"
  sudo -n mkdir -p "$mnt/root/sr-files"
  sudo -n cp "$HERE"/linux/* "$mnt/root/sr-files/"
  sudo -n cp "$SSH_KEY.pub" "$mnt/root/sr-files/authorized_keys"
  sudo -n arch-chroot "$mnt" /root/sr-files/chroot-setup.sh "$GUEST_USER" "$GUEST_IP/24"
  sudo -n ln -sf ../run/systemd/resolve/stub-resolv.conf "$mnt/etc/resolv.conf"
  sudo -n cp "$mnt/boot/vmlinuz-linux" "$mnt/boot/initramfs-linux.img" "$BOOT/"
  sudo -n chown "$(id -un)" "$BOOT"/*
  sudo -n rm -f "$mnt"/boot/initramfs-linux.img   # the guest boots the copy in $BOOT; keep the image lean
  sudo -n fstrim "$mnt" || true
  cleanup
  mountpoint -q "$mnt" && die "$mnt is still mounted; not converting"
  loop=""; trap - EXIT
  qemu-img convert -p -O qcow2 "$raw" "$DISK"
  rm -f "$raw" "$conf"; rmdir "$mnt" 2>/dev/null || true
  ls -lsh "$DISK"
}

# start gnome|kde [extra kernel args]    headless; VNC on 127.0.0.1:5931
cmd_start() {
  local session="${1:-}"; [[ $session == gnome || $session == kde ]] || die "usage: start gnome|kde"
  shift
  if running; then echo "already running (pid $(cat "$PIDFILE"))"; return 0; fi
  [[ -f $DISK && -f $BOOT/vmlinuz-linux ]] || die "no image (run 'linux.sh create' first)"
  ip link show "$TAP" >/dev/null 2>&1 || cmd_net_up >/dev/null
  rm -f "$QMP"
  # REMOTE_LINUX_VM_QEMU_EXTRA: more qemu arguments, e.g. a second head: "-device bochs-display,id=vga2,xres=1920,yres=1080"
  local extra=(); read -ra extra <<<"${REMOTE_LINUX_VM_QEMU_EXTRA:-}"
  qemu-system-x86_64 -name "$REMOTE_LINUX_VM_NAME" \
    -machine q35,accel=kvm -cpu host -smp 4 -m 6144 \
    -kernel "$BOOT/vmlinuz-linux" -initrd "$BOOT/initramfs-linux.img" \
    -append "root=/dev/vda rw console=ttyS0 loglevel=4 sr.session=$session $*" \
    -drive "file=$DISK,if=none,id=hd0,format=qcow2,cache=writeback,discard=unmap" \
    -device virtio-blk-pci,drive=hd0 \
    -device "VGA,vgamem_mb=64,xres=$RES_X,yres=$RES_Y" \
    -audiodev none,id=snd0 -device intel-hda -device hda-duplex,audiodev=snd0 \
    -device qemu-xhci -device usb-tablet -device usb-kbd \
    -netdev user,id=nat0 -device virtio-net-pci,netdev=nat0,mac=52:54:00:12:34:57 \
    -netdev "tap,id=br0,ifname=$TAP,script=no,downscript=no" -device virtio-net-pci,netdev=br0,mac=52:54:00:15:01:14 \
    -serial "file:$VMDIR/console.log" \
    -display none -vnc "127.0.0.1:$VNC_DISPLAY" \
    -qmp "unix:$QMP,server=on,wait=off" -pidfile "$PIDFILE" "${extra[@]}" -daemonize
  echo "started $session (pid $(cat "$PIDFILE")), VNC 127.0.0.1:$((5900+VNC_DISPLAY)), guest $GUEST_IP"
}

cmd_stop() {
  if ! running; then echo "not running"; return 0; fi
  local pid; pid=$(cat "$PIDFILE")
  # The desktops hold the power-key inhibitor, so poweroff over ssh is the reliable way; ACPI is the fallback.
  ssh "${ssh_opts[@]}" "$GUEST_USER@$GUEST_IP" 'sudo systemctl poweroff' >/dev/null 2>&1 || qmp '{"execute":"system_powerdown"}' >/dev/null
  for _ in $(seq 1 120); do
    if ! kill -0 "$pid" 2>/dev/null; then echo "stopped"; return 0; fi
    sleep 1
  done
  die "guest did not shut down within 120 s ('linux.sh qmp {\"execute\":\"quit\"}' is the last resort)"
}

cmd_status() {
  if running; then
    echo "vm: running (pid $(cat "$PIDFILE")), VNC 127.0.0.1:$((5900+VNC_DISPLAY)), cmdline session: $(tr ' ' '\n' < "/proc/$(cat "$PIDFILE")/cmdline" | tr '\0' '\n' | sed -n 's/.*sr\.session=\([a-z]*\).*/\1/p' | head -1)"
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
cmd_deploy() {
  [[ $# -eq 1 && -f $1 ]] || die "usage: deploy <local-binary>"
  local n; n=$(basename "$1")
  ssh "${ssh_opts[@]}" "$GUEST_USER@$GUEST_IP" 'mkdir -p bin'
  scp "${ssh_opts[@]}" "$1" "$GUEST_USER@$GUEST_IP:bin/$n"
  ssh "${ssh_opts[@]}" "$GUEST_USER@$GUEST_IP" "chmod +x bin/$n; echo \$HOME/bin/$n"
}

cmd_screenshot() { # screenshot FILE.png [DEVICE-ID]   (the id of a second head, e.g. vga2)
  [[ $# -ge 1 && $# -le 2 ]] || die "usage: screenshot <file.png> [device-id]"
  running || die "VM is not running"
  local ppm="$VMDIR/screendump.ppm" dev=""
  [[ -n ${2:-} ]] && dev=",\"device\":\"$2\""
  qmp "{\"execute\":\"screendump\",\"arguments\":{\"filename\":\"$ppm\"$dev}}" >/dev/null
  magick "$ppm" "$1"; rm -f "$ppm"
  echo "$1"
}

cmd_click() { # click X Y [left|right|middle]   screen pixels
  [[ $# -ge 2 ]] || die "usage: click X Y [left|right|middle]"
  running || die "VM is not running"
  # REMOTE_LINUX_VM_DESKTOP_W: the logical desktop width when a second head widens it (the tablet spans all of it)
  local ax=$(( $1 * 32767 / (${REMOTE_LINUX_VM_DESKTOP_W:-$RES_X}-1) )) ay=$(( $2 * 32767 / (RES_Y-1) )) b="${3:-left}"
  # The tablet reports nothing when its position doesn't change, and something else (a
  # remote host's virtual pointer) may have moved the pointer since: nudge it first.
  local nx=$(( ax > 100 ? ax - 60 : ax + 60 ))
  qmp "{\"execute\":\"input-send-event\",\"arguments\":{\"events\":[{\"type\":\"abs\",\"data\":{\"axis\":\"x\",\"value\":$nx}},{\"type\":\"abs\",\"data\":{\"axis\":\"y\",\"value\":$ay}}]}}" \
      "{\"execute\":\"input-send-event\",\"arguments\":{\"events\":[{\"type\":\"abs\",\"data\":{\"axis\":\"x\",\"value\":$ax}},{\"type\":\"abs\",\"data\":{\"axis\":\"y\",\"value\":$ay}}]}}" \
      "{\"execute\":\"input-send-event\",\"arguments\":{\"events\":[{\"type\":\"btn\",\"data\":{\"down\":true,\"button\":\"$b\"}}]}}" \
      "{\"execute\":\"input-send-event\",\"arguments\":{\"events\":[{\"type\":\"btn\",\"data\":{\"down\":false,\"button\":\"$b\"}}]}}" >/dev/null
}

cmd_key() { # key ctrl-alt-t | ret | ...   (QMP qcodes joined by -)
  [[ $# -eq 1 ]] || die "usage: key COMBO"
  running || die "VM is not running"
  local keys="" k
  IFS=- read -ra parts <<<"$1"
  for k in "${parts[@]}"; do keys+="{\"type\":\"qcode\",\"data\":\"$k\"},"; done
  qmp "{\"execute\":\"send-key\",\"arguments\":{\"keys\":[${keys%,}]}}" >/dev/null
}

cmd_type() { # type TEXT   ASCII only
  [[ $# -ge 1 ]] || die "usage: type TEXT"
  local text="$*" i c q shift
  for (( i=0; i<${#text}; i++ )); do
    c="${text:i:1}"; shift=0
    case "$c" in
      [a-z0-9]) q=$c ;;
      [A-Z]) q=${c,,}; shift=1 ;;
      ' ') q=spc ;; $'\n') q=ret ;; $'\t') q=tab ;;
      '-') q=minus ;; '=') q=equal ;; '[') q=bracket_left ;; ']') q=bracket_right ;;
      ';') q=semicolon ;; "'") q=apostrophe ;; '`') q=grave_accent ;; '\') q=backslash ;;
      ',') q=comma ;; '.') q=dot ;; '/') q=slash ;;
      '_') q=minus; shift=1 ;; '+') q=equal; shift=1 ;; '{') q=bracket_left; shift=1 ;; '}') q=bracket_right; shift=1 ;;
      ':') q=semicolon; shift=1 ;; '"') q=apostrophe; shift=1 ;; '|') q=backslash; shift=1 ;;
      '<') q=comma; shift=1 ;; '>') q=dot; shift=1 ;; '?') q=slash; shift=1 ;; '~') q=grave_accent; shift=1 ;;
      '!') q=1; shift=1 ;; '@') q=2; shift=1 ;; '#') q=3; shift=1 ;; '$') q=4; shift=1 ;; '%') q=5; shift=1 ;;
      '^') q=6; shift=1 ;; '&') q=7; shift=1 ;; '*') q=8; shift=1 ;; '(') q=9; shift=1 ;; ')') q=0; shift=1 ;;
      *) die "type: unsupported character '$c'" ;;
    esac
    if (( shift )); then cmd_key "shift-$q"; else cmd_key "$q"; fi
    sleep 0.03
  done
}

cmd_snapshot() { [[ $# -eq 1 ]] || die "usage: snapshot <name>"; if running; then die "stop the VM first"; fi; qemu-img snapshot -c "$1" "$DISK"; }
cmd_revert()   { [[ $# -eq 1 ]] || die "usage: revert <name>";   if running; then die "stop the VM first"; fi; qemu-img snapshot -a "$1" "$DISK"; }

sub="${1:-}"; shift || true
# One VM, several builders: same lock rules as win.sh (stale after 90 minutes).
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
  create) cmd_create ;;
  start) cmd_start "$@" ;;
  stop) cmd_stop ;;
  status) cmd_status ;;
  ssh) cmd_ssh "$@" ;;
  scp-to) cmd_scp_to "$@" ;;
  scp-from) cmd_scp_from "$@" ;;
  deploy) cmd_deploy "$@" ;;
  screenshot) cmd_screenshot "$@" ;;
  click) cmd_click "$@" ;;
  key) cmd_key "$@" ;;
  type) cmd_type "$@" ;;
  snapshot) cmd_snapshot "$@" ;;
  revert) cmd_revert "$@" ;;
  qmp) qmp "$@" ;;
  lock) cmd_lock "$@" ;;
  unlock) cmd_unlock "$@" ;;
  who) cmd_who ;;
  *) echo "usage: linux.sh net-up|net-down|create|start gnome|kde|stop|status|ssh [cmd]|scp-to L R|scp-from R L|deploy BIN|screenshot F.png|click X Y [btn]|key COMBO|type TEXT|snapshot N|revert N|lock O|unlock O|who|qmp JSON" >&2; exit 2 ;;
esac
