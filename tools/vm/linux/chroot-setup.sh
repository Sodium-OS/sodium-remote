#!/bin/bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# Runs inside arch-chroot on the new guest root. /root/sr-files holds the files from linux/.
#   chroot-setup.sh USER ADDRESS/24   (linux.sh create passes config.sh's values)
set -euo pipefail
F=/root/sr-files
U=${1:?user}
ADDR=${2:?address/24}
echo sr-linux > /etc/hostname
ln -sf /usr/share/zoneinfo/UTC /etc/localtime
sed -i 's/^#en_US.UTF-8/en_US.UTF-8/; s/^#ja_JP.UTF-8/ja_JP.UTF-8/' /etc/locale.gen
locale-gen >/dev/null
echo LANG=en_US.UTF-8 > /etc/locale.conf
echo KEYMAP=us > /etc/vconsole.conf
echo '/dev/vda / ext4 rw,relatime 0 1' > /etc/fstab

# no firmware in the guest (virtual hardware only)
pacman -Rdd --noconfirm $(pacman -Qq | grep '^linux-firmware') 2>/dev/null || true

# initramfs: no autodetect (the build host's hardware is not the guest's), virtio built in
sed -i 's/^MODULES=.*/MODULES=(virtio_blk virtio_pci virtio_gpu virtio_net ext4)/;
        s/^HOOKS=.*/HOOKS=(base udev modconf block filesystems keyboard)/' /etc/mkinitcpio.conf
sed -i "s/^PRESETS=.*/PRESETS=('default')/" /etc/mkinitcpio.d/linux.preset
rm -f /boot/initramfs-linux-fallback.img
mkinitcpio -p linux

# user
useradd -m -u 1000 -G wheel -s /bin/bash "$U"
H=$(getent passwd "$U" | cut -d: -f6)
echo "$U ALL=(ALL) NOPASSWD: ALL" > /etc/sudoers.d/10-sr; chmod 440 /etc/sudoers.d/10-sr
install -d -m700 -o "$U" -g "$U" "$H/.ssh" "$H/.config"
install -d -o "$U" -g "$U" "$H/bin"
install -m600 -o "$U" -g "$U" $F/authorized_keys "$H/.ssh/authorized_keys"
cat > /etc/ssh/sshd_config.d/10-sr.conf <<'X'
PasswordAuthentication no
KbdInteractiveAuthentication no
X

# network
mkdir -p /etc/systemd/network
cat > /etc/systemd/network/10-bridge.network <<X
[Match]
MACAddress=52:54:00:15:01:14
[Network]
Address=$ADDR
LinkLocalAddressing=no
X
cat > /etc/systemd/network/20-nat.network <<'X'
[Match]
MACAddress=52:54:00:12:34:57
[Network]
DHCP=ipv4
X
systemctl enable systemd-networkd systemd-resolved sshd
systemctl set-default graphical.target
systemctl --global enable pipewire.socket pipewire-pulse.socket wireplumber.service

# session chooser
install -m755 $F/sr-session /usr/local/lib/sr-session 2>/dev/null || { mkdir -p /usr/local/lib; install -m755 $F/sr-session /usr/local/lib/sr-session; }
install -m644 $F/sr-session.service /etc/systemd/system/sr-session.service
systemctl enable sr-session.service
systemctl disable gdm sddm 2>/dev/null || true

# GNOME: GDM autologin, dconf defaults
mkdir -p /etc/gdm /etc/dconf/profile /etc/dconf/db/local.d
cat > /etc/gdm/custom.conf <<X
[daemon]
AutomaticLoginEnable=True
AutomaticLogin=$U
X
printf 'user-db:user\nsystem-db:local\n' > /etc/dconf/profile/user
install -m644 $F/00-sr.dconf /etc/dconf/db/local.d/00-sr
dconf update

# KDE: SDDM autologin (Wayland session), no lock/suspend/notification popups
mkdir -p /etc/sddm.conf.d
cat > /etc/sddm.conf.d/10-sr.conf <<X
[General]
DisplayServer=wayland
[Autologin]
User=$U
Session=plasma.desktop
Relogin=true
X
install -m644 $F/kscreenlockerrc $F/powerdevilrc $F/plasmanotifyrc /etc/xdg/

# trim
pacman -Scc --noconfirm >/dev/null
rm -rf /var/cache/pacman/pkg/* /var/lib/pacman/sync/* /usr/share/doc /usr/share/man /usr/share/info /root/sr-files
echo chroot-setup done
