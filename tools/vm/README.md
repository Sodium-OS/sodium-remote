# Windows 11 test VM for Sodium Remote

A Windows 11 guest on this PC (QEMU/KVM, no libvirt) for the cross-platform run: the Windows host and client engine are tested here. Everything lives in `$REMOTE_VM_DIR` (default `~/.local/share/sodium-remote-test-vm/`); nothing is in `/tmp` and nothing from there is in the repo.

## Settings

Every name, path and address below is a default from `config.sh`. Change one in the environment or in `tools/vm/local.env` (gitignored; `local.env.example` lists them all): `REMOTE_VM_DIR`, `REMOTE_VM_USER`, `REMOTE_VM_COMPUTER`, `REMOTE_VM_NAME`, `REMOTE_VM_BRIDGE`, `REMOTE_VM_TAP`, `REMOTE_VM_NET`, `REMOTE_VM_MAC`, `REMOTE_VM_SSH_KEY`, `REMOTE_VM_GUEST_DIR`, `REMOTE_VM_VNC_DISPLAY`. The default subnet is TEST-NET-2 (198.51.100.0/24, RFC 5737), which nothing routes; set `REMOTE_VM_NET` to the first three octets of any private /24 that none of your real networks uses.

## Use

```
remote/tools/vm/win.sh net-up | net-down      # host-only bridge + tap (sudo ip, no other changes)
remote/tools/vm/win.sh start                   # headless; boots to the test account's desktop on its own
remote/tools/vm/win.sh status
remote/tools/vm/win.sh ssh 'ver; whoami'       # PowerShell on the guest, key auth
remote/tools/vm/win.sh scp-to ./app.exe 'C:/Users/remote/app.exe'
remote/tools/vm/win.sh scp-from 'C:/Users/remote/out.txt' .
remote/tools/vm/win.sh screenshot shot.png     # QMP screendump -> PNG
remote/tools/vm/win.sh stop                    # ACPI shutdown, waits up to 240 s
remote/tools/vm/win.sh snapshot clean | revert clean   # qcow2 snapshots, VM stopped
```

`create.sh` builds the VM from the ISOs in `$REMOTE_VM_DIR` (about 15 minutes, unattended). It leaves the VM running; stop it and take the `clean` snapshot.

## Addresses

| What | Value |
|---|---|
| Host side of the bridge | `br-remote-test`, 198.51.100.1/24 (host-only: no forwarding, no NAT; your real interfaces and the default route are untouched) |
| Guest on the bridge | 198.51.100.10/24, static, no gateway, adapter "Bridge", MAC 52:54:00:15:01:0a, network profile Private |
| Guest internet | adapter "Internet", QEMU user-mode NAT (its default addressing), outbound only |
| SSH | `remote@198.51.100.10`, key `~/.ssh/sodium-remote-test-vm`, default shell PowerShell |
| VNC | `127.0.0.1:5930` (loopback only; `vncviewer 127.0.0.1:5930`), no password |
| QMP | `$REMOTE_VM_DIR/qmp.sock` |
| Login | local account `remote`, password in `$REMOTE_VM_DIR/credentials` (mode 600); automatic logon is on |

The bridge is for Remote's tests: mDNS (UDP 5353) and direct UDP/TCP between this PC and the guest. The guest firewall allows ICMP, UDP 5353 and everything from 198.51.100.0/24 on all profiles. The bridge and tap do not survive a reboot of this PC; `win.sh start` recreates them (`net-up`).

## How it is built

- q35, KVM, 6 vCPUs, 8 GB RAM, `Skylake-Client-v4` with the common Hyper-V enlightenments (`-cpu host` froze the guest on a hybrid P/E-core Intel CPU), clock in local time.
- OVMF Secure-Boot-capable code from `/usr/share/edk2/x64/` with a private copy of the vars (`OVMF_VARS.fd`, no keys enrolled, so Secure Boot is capable but off), swtpm TPM 2.0 in `tpm/`.
- SATA disk (`disk.qcow2`, 64 GB sparse) and e1000e NICs: both have inbox Windows drivers, so no virtio drivers are needed. `virtio-win.iso` is attached as a CD during install only, in case a driver is wanted later.
- Display: the standard VGA device (Windows uses its Microsoft Basic Display Adapter) with a 1920x1080 EDID. Audio: Intel HDA duplex with `-audiodev none`, which gives Windows a render endpoint for WASAPI loopback and plays nowhere.
- Windows 11 Enterprise Evaluation (see below), local account `remote`, no Microsoft account, automatic logon, no lock screen or sleep, OpenSSH Server with a dedicated key, Windows Update paused for 35 days with no reboot while signed in.
- `autounattend.xml` + `vm-setup.ps1` + `vm-config.ps1` (+ the public key) go on a small ISO (`unattend.iso`) that Windows Setup finds by itself; `@@PASSWORD@@`, `@@USER@@` and `@@COMPUTER@@` are replaced at build time. The setup log is `C:\remote-test\setup.log` in the guest.

## Choices and limits

- **ISO.** Microsoft's consumer download API answered "Sentinel marked this request as rejected" to a scripted request, so the VM uses the official Windows 11 Enterprise Evaluation ISO from Microsoft's own `go.microsoft.com/fwlink` link (en-US, x64, build 26300.9457, the fwlink on Microsoft's Evaluation Center page). Microsoft publishes no SHA-256 for it; the one we computed is in `$REMOTE_VM_DIR/win11-enterprise-eval.iso.sha256`. The evaluation is unactivated and expires after 90 days; rebuild with `create.sh` then (or use a retail ISO via `WIN_ISO=`).
- **No GPU.** The guest has no GPU: rendering is WARP (software), there is no hardware encode or decode, and no D3D11 video acceleration. Anything that measures performance here tells you about correctness, not speed. DXGI Desktop Duplication has to be verified on the Basic Display Adapter by the host work.
- **Password auth** over SSH is left on (the default); only the bridge reaches it, because the NAT NIC takes no inbound connections.
- Windows Update is paused by registry values; the Settings app may show them as policy-managed.
- The Bridge NIC has no gateway, so Windows files it under "Unidentified network"; a startup task (`RemoteTestBridgePrivate`) keeps it Private.
- Snapshots are internal qcow2 snapshots: stop the VM first. `clean` is the state after the install and a clean shutdown.

## Sharing the VM between builders

There is one VM and several builders. Before anything that uses the guest's desktop, or that reboots, stops, snapshots or reverts it, take the lock with `win.sh lock <your-name>`. When you're done, release it with `win.sh unlock <your-name>`.
- `win.sh who` shows who holds the lock.
- A hold older than 90 minutes is stale and may be taken over.
- Keep each hold short, about 45 minutes at most.
- Leave the VM running when you unlock.
- Work in your own guest folder, `C:\remote-test\<your-name>\`, and stop only your own processes.
- **A freshly copied app meets the firewall prompt.** Windows asks "Do you want to allow public and private networks to access this app?" the first time a program at a new path listens. The prompt stays on screen over every later screenshot, and leaving or cancelling it adds two *Block* rules (Public) for that path, which beat any Allow rule: the This computer runs then see the probe "Offline". Before a run of `integration/all.sh`, add an Allow rule for `C:\remote-test\integration\SodiumRemote\SodiumRemote.exe` (`New-NetFirewallRule -Direction Inbound -Action Allow -Profile Any -Program …`); if the prompt already showed, press its Cancel and delete the Block rules for that path. Remove what you added when you unlock.

## Linux test VM (arch-desktops)

An Arch Linux guest with both GNOME and KDE Plasma (Wayland) for the generic Linux host: xdg-desktop-portal RemoteDesktop / ScreenCast / Clipboard over PipeWire, libei input. Control script: `remote/tools/vm/linux.sh`. Its settings come from `config.sh` (and `local.env`) like the Windows VM's: `REMOTE_LINUX_VM_*` for its folder, account, tap, key and VNC display; the bridge and the /24 are the Windows VM's (`REMOTE_VM_BRIDGE`, `REMOTE_VM_NET`), the guest is `.20`. Everything lives in `$REMOTE_LINUX_VM_DIR` (nothing in `/tmp`, nothing in git). The Windows VM is untouched.

```
remote/tools/vm/linux.sh create                # once: pacstrap an image (~10 min, needs sudo -n and ~15 GB of host disk while building)
remote/tools/vm/linux.sh start gnome|kde       # headless; the desktop is chosen on the kernel command line
remote/tools/vm/linux.sh status | stop         # stop = `systemctl poweroff` over ssh, ACPI as fallback
remote/tools/vm/linux.sh ssh 'busctl --user ...'   # as the guest account, key auth (XDG_RUNTIME_DIR=/run/user/1000 for --user tools)
remote/tools/vm/linux.sh scp-to L R | scp-from R L | deploy ./binary   # deploy -> ~/bin/<name> in the guest, chmod +x
remote/tools/vm/linux.sh screenshot f.png [ID]  # QMP screendump -> PNG (ID: a second head, e.g. vga2)
remote/tools/vm/linux.sh click X Y [button]    # screen pixels (REMOTE_LINUX_VM_DESKTOP_W widens the range when a second head does); key ctrl-alt-t | ret; type 'text' (ASCII)
remote/tools/vm/linux.sh snapshot N | revert N # VM stopped; `clean` = first boot of each desktop + clean stop
remote/tools/vm/linux.sh lock O | unlock O | who   # same rules as win.sh, lock dir $REMOTE_LINUX_VM_DIR/lock.d
remote/tools/vm/linux.sh net-up | net-down     # its tap on the bridge; net-down leaves the bridge if another tap is on it
```

| What | Value |
|---|---|
| Host side of the bridge | `$REMOTE_VM_BRIDGE`, `$REMOTE_VM_NET.1/24` (shared with the Windows VM; host-only, no forwarding, no NAT); the guest's tap is `$REMOTE_LINUX_VM_TAP` |
| Guest on the bridge | `$REMOTE_VM_NET.20/24`, static, no gateway, MAC 52:54:00:15:01:14 (systemd-networkd), hostname `sr-linux` |
| Guest internet | second NIC, QEMU user-mode NAT (DHCP, outbound only), MAC 52:54:00:12:34:57 |
| SSH | `$REMOTE_LINUX_VM_USER` at the guest's address, key `$REMOTE_LINUX_VM_SSH_KEY` (password login off; the account has no password, passwordless sudo) |
| VNC | `127.0.0.1:5931` (loopback only, no password) |
| QMP | `qmp.sock` in the VM folder; serial console log `console.log` |
| Files | `arch-desktops.qcow2` (24 GB virtual, sparse, ~5 GB used), `boot/vmlinuz-linux` + `boot/initramfs-linux.img`, `evidence/` |

The end-to-end test that drives it is `remote/host-desktop/tests/linux/e2e.sh gnome|kde [--fresh]` ([host-linux.md](../../docs/reference/host-linux.md), Testing).

Several monitors: `REMOTE_LINUX_VM_QEMU_EXTRA="-device bochs-display,id=vga2,xres=1920,yres=1080" linux.sh start gnome|kde` adds a second head (a second DRM card, `Virtual-2`). Scale it in the guest (GNOME: `org.gnome.Mutter.DisplayConfig.ApplyMonitorsConfig`; KDE: `kscreen-doctor output.Virtual-2.scale.2`). The usb-tablet only reaches the first head: answer a dialog on the second with the keyboard, and set `REMOTE_LINUX_VM_DESKTOP_W` to the logical width of both heads for `click`.

### How it is built

`linux.sh create`: sparse raw image, ext4 on the whole device (no partition table), `pacstrap -K` from the host's configured mirrors (no host package cache, no ISO), `linux/chroot-setup.sh USER ADDRESS/24` inside `arch-chroot`, then `qemu-img convert` to qcow2. Packages: base, linux, openssh, mesa, pipewire + pipewire-pulse/alsa + wireplumber, libei, ffmpeg, openh264, opus, wl-clipboard, python-gobject, gtk4, noto-fonts + noto-fonts-cjk, xorg-xwayland; GNOME (gnome-shell, gnome-session, gdm, xdg-desktop-portal-gnome, ptyxis); KDE (plasma-desktop, plasma-workspace, kwin, sddm, xdg-desktop-portal-kde, konsole). No first-run wizards are installed (no gnome-initial-setup, gnome-tour, plasma-welcome). No linux-firmware, docs or man pages; the initramfs is a single non-autodetect image (virtio_blk/pci/gpu/net, ext4). Guest-side files are in `remote/tools/vm/linux/`.

### How a session is chosen

QEMU direct-kernel-boots `boot/vmlinuz-linux` with `-append "root=/dev/vda rw ... sr.session=gnome|kde"`. `sr-session.service` (`/usr/local/lib/sr-session`) reads `/proc/cmdline` and starts `gdm.service` (gnome) or `sddm.service` (kde); neither is enabled otherwise. GDM and SDDM autologin as the guest account into the Wayland GNOME session / Plasma Wayland session, so logind, the user systemd manager, the session D-Bus and the portals are the normal ones. Lock screen, blanking, idle suspend and notification banners are off (GNOME: `/etc/dconf/db/local.d/00-sr`; KDE: `/etc/xdg/kscreenlockerrc`, `powerdevilrc`, `plasmanotifyrc`). GNOME starts on its Overview screen (press Escape).

### Limits

- No GPU: the display is the QEMU standard VGA (1920x1080, `bochs` driver) and Mesa renders with llvmpipe. There is no VA-API and no DMA-BUF hardware path; PipeWire video from the portal is memfd/shm-backed. This VM verifies correctness, not speed. (`virtio-vga` is not available in this host's QEMU build; the standard VGA is used instead.)
- Wayland only, one desktop per boot (4 vCPUs, 6 GB RAM, `-cpu host`).
- The audio device is Intel HDA with `-audiodev none`: PipeWire has a real sink, nothing is played.
- The kernel comes from the image's own `linux` package, copied to `boot/` at build time; rebuild (`create`) to update it.
