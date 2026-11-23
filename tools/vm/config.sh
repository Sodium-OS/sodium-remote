# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# Settings shared by the VM scripts (sourced, not run). Every value has a neutral default;
# change one in the environment or in tools/vm/local.env (gitignored, a shell fragment of
# VAR=value lines; local.env.example lists them all).

_vm_cfg_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck disable=SC1091
[[ -f $_vm_cfg_dir/local.env ]] && source "$_vm_cfg_dir/local.env"

# Where the VM's disk, firmware variables, TPM state, credentials and sockets live.
: "${REMOTE_VM_DIR:=$HOME/.local/share/sodium-remote-test-vm}"
# The guest's local administrator account, and the guest's computer name.
: "${REMOTE_VM_USER:=remote}"
: "${REMOTE_VM_COMPUTER:=REMOTE-TEST-WIN}"
# QEMU's process name (-name).
: "${REMOTE_VM_NAME:=remote-test-win}"
# The host-only bridge and its tap device on this machine.
: "${REMOTE_VM_BRIDGE:=br-remote-test}"
: "${REMOTE_VM_TAP:=tap-remote-test}"
# The first three octets of the host-only /24: this machine is .1, the guest .10. The
# default is TEST-NET-2 (RFC 5737), which nothing routes; any private /24 that no real
# network of yours uses works too.
: "${REMOTE_VM_NET:=198.51.100}"
# The guest's bridge NIC (QEMU's own MAC prefix; any locally administered MAC works).
: "${REMOTE_VM_MAC:=52:54:00:15:01:0a}"
# The key that logs in to the guest over SSH (created by create.sh when missing).
: "${REMOTE_VM_SSH_KEY:=$HOME/.ssh/sodium-remote-test-vm}"
# The guest's working folder, with backslashes (the guest's own folders live under it).
: "${REMOTE_VM_GUEST_DIR:=C:\\remote-test}"
# Loopback VNC display number (port 5900 + this).
: "${REMOTE_VM_VNC_DISPLAY:=30}"

# The Linux test VM (linux.sh: Arch with GNOME and KDE Plasma), on the same host-only /24
# as the guest .20, with its own tap, folder, account and key.
: "${REMOTE_LINUX_VM_DIR:=$HOME/.local/share/sodium-remote-test-vm-linux}"
: "${REMOTE_LINUX_VM_USER:=remote}"
: "${REMOTE_LINUX_VM_NAME:=remote-test-linux}"
: "${REMOTE_LINUX_VM_TAP:=tap-remote-lx}"
: "${REMOTE_LINUX_VM_SSH_KEY:=$HOME/.ssh/sodium-remote-test-vm-linux}"
: "${REMOTE_LINUX_VM_VNC_DISPLAY:=31}"
LX_GUEST_IP=$REMOTE_VM_NET.20

BR=$REMOTE_VM_BRIDGE
TAP=$REMOTE_VM_TAP
BR_IP=$REMOTE_VM_NET.1
GUEST_IP=$REMOTE_VM_NET.10
GUEST_USER=$REMOTE_VM_USER
SSH_KEY=$REMOTE_VM_SSH_KEY
VNC_DISPLAY=$REMOTE_VM_VNC_DISPLAY
# The guest's working folder, as PowerShell writes it (GD) and as scp writes it (GF).
GD=$REMOTE_VM_GUEST_DIR
GF=${GD//\\//}
