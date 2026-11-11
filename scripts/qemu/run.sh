#!/usr/bin/env bash
# Boot the m3 test image in QEMU with a virtio-gpu display, USB keyboard and
# tablet. Runs on Linux (KVM) or macOS on Apple Silicon (HVF).
#
#   scripts/qemu/run.sh <image.qcow2> [state-dir]
#
# Opens a window (cocoa on macOS, gtk on Linux). For automation run headless
# with QEMU_DISPLAY=none and control it through the QMP socket, e.g.:
#   scripts/qemu/qmp.py <state-dir>/qmp.sock screendump out.png
set -euo pipefail
IMG=$1
STATE=${2:-$(dirname "$IMG")/qemu-state}
mkdir -p "$STATE"
case "$(uname -s)" in
    Darwin) ACCEL=hvf; FW=$(brew --prefix qemu)/share/qemu/edk2-aarch64-code.fd; WINDOW=cocoa ;;
    *) ACCEL=kvm; FW=/usr/share/edk2/aarch64/QEMU_CODE.fd; WINDOW=gtk ;;
esac
DISPLAY_MODE=${QEMU_DISPLAY:-$WINDOW}
[ -f "$STATE/vars.fd" ] || dd if=/dev/zero of="$STATE/vars.fd" bs=1m count=64 2>/dev/null
exec qemu-system-aarch64 \
    -M virt -accel "$ACCEL" -cpu host -smp 4 -m 4096 \
    -drive if=pflash,format=raw,readonly=on,file="$FW" \
    -drive if=pflash,format=raw,file="$STATE/vars.fd" \
    -drive file="$IMG",if=virtio,format=qcow2 \
    -device virtio-gpu-pci,xres=1600,yres=1000 \
    -device qemu-xhci -device usb-kbd -device usb-tablet \
    -nic user,model=virtio-net-pci,hostfwd=tcp:127.0.0.1:"${QEMU_SSH_PORT:-2222}"-:22 \
    -display "$DISPLAY_MODE" \
    -qmp unix:"$STATE/qmp.sock",server,nowait \
    -serial file:"$STATE/serial.log"
