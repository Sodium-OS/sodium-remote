#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# Build the Windows 11 test VM from the ISOs in $REMOTE_VM_DIR (unattended). See README.md.
#   create.sh            expects win11.iso (or win11-enterprise-eval.iso) and virtio-win.iso there
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
# shellcheck disable=SC1091
source "$HERE/config.sh"
VMDIR="$REMOTE_VM_DIR"
WIN="$HERE/win.sh"
ISO="${WIN_ISO:-$VMDIR/win11-enterprise-eval.iso}"
die() { echo "create.sh: $*" >&2; exit 1; }

[[ -f $ISO ]] || die "missing $ISO"
[[ -f $VMDIR/virtio-win.iso ]] || die "missing $VMDIR/virtio-win.iso"
[[ ! -f $VMDIR/disk.qcow2 ]] || die "$VMDIR/disk.qcow2 already exists; remove it to rebuild"
[[ -f $VMDIR/win11-enterprise-eval.iso.sha256 ]] && (cd "$VMDIR" && sha256sum -c win11-enterprise-eval.iso.sha256)

# credentials and the SSH key (never in the repo)
if [[ ! -f $VMDIR/credentials ]]; then
  pw="$(openssl rand -base64 24 | tr -d '/+=\n' | cut -c1-20)-Aa1"
  (umask 077; printf 'user=%s\npassword=%s\n' "$GUEST_USER" "$pw" > "$VMDIR/credentials")
fi
chmod 600 "$VMDIR/credentials"
PW="$(sed -n 's/^password=//p' "$VMDIR/credentials")"
[[ -f $SSH_KEY ]] || ssh-keygen -q -t ed25519 -N '' -C sodium-remote-test-vm -f "$SSH_KEY"

# unattend ISO: autounattend.xml + setup script + the public key
STAGE="$VMDIR/unattend-stage"
rm -rf "$STAGE"; mkdir -p "$STAGE"; chmod 700 "$STAGE"
sed -e "s|@@PASSWORD@@|$PW|g" -e "s|@@USER@@|$GUEST_USER|g" -e "s|@@COMPUTER@@|$REMOTE_VM_COMPUTER|g" \
  "$HERE/autounattend.xml" > "$STAGE/autounattend.xml"
cp "$HERE/vm-setup.ps1" "$STAGE/"
# what vm-setup.ps1 needs to know, in PowerShell
cat > "$STAGE/vm-config.ps1" <<CFG
\$Cfg = @{ Dir = '$GD'; Net = '$REMOTE_VM_NET'; Mac = '$REMOTE_VM_MAC' }
CFG
cp "$SSH_KEY.pub" "$STAGE/test_vm.pub"
xorriso -as mkisofs -quiet -J -r -V UNATTEND -o "$VMDIR/unattend.iso" "$STAGE"
chmod 600 "$VMDIR/unattend.iso"
rm -rf "$STAGE"

# disk (sparse), UEFI vars, TPM
qemu-img create -f qcow2 "$VMDIR/disk.qcow2" 64G
cp /usr/share/edk2/x64/OVMF_VARS.4m.fd "$VMDIR/OVMF_VARS.fd"

"$WIN" net-up
"$WIN" start --install "$ISO" "$VMDIR/unattend.iso"

# "Press any key to boot from CD": tap space for the first 40 s
for _ in $(seq 1 40); do
  "$WIN" qmp '{"execute":"send-key","arguments":{"keys":[{"type":"qcode","data":"spc"}]}}' >/dev/null || true
  sleep 1
done

echo "installing; waiting for SSH on $GUEST_IP (up to 90 min)"
for i in $(seq 1 540); do
  if "$WIN" ssh "if (Test-Path $GD\\setup.done) { 'ready' }" 2>/dev/null | grep -q ready; then
    echo "guest is set up"; break
  fi
  sleep 10
  [[ $i -lt 540 ]] || die "timed out; look at it with VNC 127.0.0.1:$((5900+VNC_DISPLAY)) and $GD\\setup.log"
done

"$WIN" ssh "Get-Content $GD\\setup.log -Tail 5" || true
echo "install done; the VM is still running. Use 'win.sh stop' then 'win.sh snapshot clean'."
