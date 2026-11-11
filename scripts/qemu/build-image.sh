#!/usr/bin/env bash
# Build a bootable Arch Linux ARM disk image with m3 installed, for testing
# the real DRM/KMS session path in QEMU (virtio-gpu).
#
# Run as root on an aarch64 Arch Linux ARM machine (the dev VM):
#   scripts/qemu/build-image.sh <m3-desktop-*.pkg.tar.zst> <output.qcow2>
#
# No loop devices or mounts are needed: the root filesystem is created with
# `mkfs.ext4 -d` from a pacstrap'd directory and the EFI partition with mtools.
# The image auto-logs-in user `m3test` (password `m3test`) into m3 via greetd.
set -euo pipefail
PKG=$(realpath "$1")
OUT=$2
WORK=${WORK:-/var/tmp/m3-image}
SIZE_MB=${SIZE_MB:-6144}
ESP_MB=300

pacman -S --noconfirm --needed arch-install-scripts gptfdisk dosfstools mtools e2fsprogs qemu-img >/dev/null

rm -rf "$WORK"
mkdir -p "$WORK/root"
ROOT="$WORK/root"

pacstrap -c -K "$ROOT" base linux-aarch64 mkinitcpio systemd-sysvcompat sudo \
    mesa seatd gtk4 gtk4-layer-shell libinput libxkbcommon libdisplay-info pam libpulse \
    greetd foot inter-font adwaita-icon-theme pipewire pipewire-pulse wireplumber \
    upower power-profiles-daemon libnotify xorg-xwayland openssh grim
pacman --root "$ROOT" --noconfirm -U "$PKG"

# Users and session.
arch-chroot "$ROOT" useradd -m -G video,input,seat m3test 2>/dev/null || arch-chroot "$ROOT" useradd -m -G video,input m3test
echo "m3test:m3test" | arch-chroot "$ROOT" chpasswd
echo "root:root" | arch-chroot "$ROOT" chpasswd
echo m3-vm > "$ROOT/etc/hostname"
# Remote control for development: an SSH key (AUTHORIZED_KEY=<file.pub>) and
# optional sample music (SAMPLE_MUSIC=<dir>). run.sh forwards SSH to
# 127.0.0.1 only.
if [ -n "${AUTHORIZED_KEY:-}" ]; then
    install -d -m700 -o 1000 -g 1000 "$ROOT/home/m3test/.ssh"
    install -m600 -o 1000 -g 1000 "$AUTHORIZED_KEY" "$ROOT/home/m3test/.ssh/authorized_keys"
    arch-chroot "$ROOT" systemctl enable sshd.service
fi
if [ -n "${SAMPLE_MUSIC:-}" ] && [ -d "$SAMPLE_MUSIC" ]; then
    install -d -o 1000 -g 1000 "$ROOT/home/m3test/Music"
    cp -r "$SAMPLE_MUSIC" "$ROOT/home/m3test/Music/"
    chown -R 1000:1000 "$ROOT/home/m3test/Music"
fi
cat > "$ROOT/etc/greetd/config.toml" <<'EOF'
[terminal]
vt = 1

# Log straight into m3 (test image only).
[initial_session]
command = "m3-session"
user = "m3test"

[default_session]
command = "agreety --cmd m3-session"
user = "greeter"
EOF
arch-chroot "$ROOT" systemctl enable greetd.service seatd.service systemd-networkd.service systemd-resolved.service
mkdir -p "$ROOT/etc/systemd/network"
printf '[Match]\nName=en*\n[Network]\nDHCP=yes\n' > "$ROOT/etc/systemd/network/20-wired.network"
# Serial console for debugging.
arch-chroot "$ROOT" systemctl enable serial-getty@ttyAMA0.service
# Journal visible on serial for boot problems; persistent for later reading.
mkdir -p "$ROOT/var/log/journal"

# Boot: systemd-boot on the ESP (directory first, copied into a FAT image).
ESPDIR="$WORK/esp"
mkdir -p "$ESPDIR/EFI/BOOT" "$ESPDIR/loader/entries"
cp "$ROOT/usr/lib/systemd/boot/efi/systemd-bootaa64.efi" "$ESPDIR/EFI/BOOT/BOOTAA64.EFI"
cp "$ROOT/boot/Image" "$ESPDIR/Image"
cp "$ROOT/boot/initramfs-linux.img" "$ESPDIR/initramfs-linux.img"
printf 'default m3.conf\ntimeout 0\n' > "$ESPDIR/loader/loader.conf"
cat > "$ESPDIR/loader/entries/m3.conf" <<'EOF'
title   m3 test image
linux   /Image
initrd  /initramfs-linux.img
options root=PARTLABEL=m3root rw console=tty1 console=ttyAMA0 quiet
EOF

# Partition table + filesystems.
RAW="$WORK/disk.raw"
truncate -s "${SIZE_MB}M" "$RAW"
sgdisk -Z -n1:2048:+${ESP_MB}M -t1:ef00 -c1:ESP -n2:0:0 -t2:8305 -c2:m3root "$RAW" >/dev/null
read -r ESP_START ESP_END < <(sgdisk -i1 "$RAW" | awk '/First sector/{s=$3} /Last sector/{e=$3} END{print s, e}')
read -r ROOT_START ROOT_END < <(sgdisk -i2 "$RAW" | awk '/First sector/{s=$3} /Last sector/{e=$3} END{print s, e}')

ESPIMG="$WORK/esp.img"
truncate -s "$(( (ESP_END - ESP_START + 1) * 512 ))" "$ESPIMG"
mkfs.fat -F32 "$ESPIMG" >/dev/null
mcopy -s -i "$ESPIMG" "$ESPDIR"/* ::/
ROOTIMG="$WORK/root.img"
truncate -s "$(( (ROOT_END - ROOT_START + 1) * 512 ))" "$ROOTIMG"
mkfs.ext4 -q -d "$ROOT" "$ROOTIMG"

dd if="$ESPIMG" of="$RAW" bs=512 seek="$ESP_START" conv=notrunc,sparse status=none
dd if="$ROOTIMG" of="$RAW" bs=512 seek="$ROOT_START" conv=notrunc,sparse status=none
qemu-img convert -c -O qcow2 "$RAW" "$OUT"
rm -rf "$WORK"
echo "built $OUT"
