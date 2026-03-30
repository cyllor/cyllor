#!/bin/bash
set -e

ARCH=${1:-aarch64}
KERNEL_ELF=${2:-target/${ARCH}-unknown-none/debug/cyllor-kernel}
IMG=target/cyllor-${ARCH}.img
LIMINE_DIR=target/limine

# Create a 64MB FAT32 disk image
dd if=/dev/zero of=${IMG} bs=1M count=64 2>/dev/null
# Create GPT with an EFI System Partition
DISK_SIZE=$(stat -f%z ${IMG} 2>/dev/null || stat -c%s ${IMG})

# Use mtools to create a FAT image directly
# First create the ESP as a FAT filesystem
ESP_IMG=target/esp.img
dd if=/dev/zero of=${ESP_IMG} bs=1M count=62 2>/dev/null
mformat -i ${ESP_IMG} -F ::

# Copy files into FAT image
mmd -i ${ESP_IMG} ::/EFI 2>/dev/null || true
mmd -i ${ESP_IMG} ::/EFI/BOOT 2>/dev/null || true

if [ "${ARCH}" = "aarch64" ]; then
    mcopy -i ${ESP_IMG} ${LIMINE_DIR}/BOOTAA64.EFI ::/EFI/BOOT/BOOTAA64.EFI
elif [ "${ARCH}" = "x86_64" ]; then
    mcopy -i ${ESP_IMG} ${LIMINE_DIR}/BOOTX64.EFI ::/EFI/BOOT/BOOTX64.EFI
fi

mcopy -i ${ESP_IMG} limine.conf ::/limine.conf
mcopy -i ${ESP_IMG} ${KERNEL_ELF} ::/kernel.elf

# Use the ESP image directly as disk image for QEMU
cp ${ESP_IMG} ${IMG}
rm ${ESP_IMG}

echo "Created ${IMG}"
