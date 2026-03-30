ARCH ?= aarch64
PROFILE ?= dev

ifeq ($(PROFILE),release)
  CARGO_FLAGS = --release
  TARGET_DIR = release
else
  CARGO_FLAGS =
  TARGET_DIR = debug
endif

TARGET_TRIPLE = $(ARCH)-unknown-none
KERNEL_ELF = target/$(TARGET_TRIPLE)/$(TARGET_DIR)/cyllor-kernel
IMG = target/cyllor-$(ARCH).img

LIMINE_DIR = target/limine

.PHONY: all build run run-gui clean limine image

all: build

limine:
	@if [ ! -d "$(LIMINE_DIR)" ]; then \
		echo "Cloning Limine..."; \
		git clone https://github.com/limine-bootloader/limine.git --branch=v8.x-binary --depth=1 $(LIMINE_DIR); \
	fi

build: limine
	cargo +nightly build --target $(TARGET_TRIPLE) $(CARGO_FLAGS) -p cyllor-kernel

image: build
	bash tools/mkimage.sh $(ARCH) $(KERNEL_ELF)

# Run with serial output only (headless)
run: image
ifeq ($(ARCH),aarch64)
	qemu-system-aarch64 \
		-M virt -cpu cortex-a72 -m 512M -smp 4 \
		-serial stdio \
		-bios /opt/homebrew/share/qemu/edk2-aarch64-code.fd \
		-drive file=$(IMG),format=raw \
		-device ramfb \
		-no-reboot -display none
else
	qemu-system-x86_64 \
		-M q35 -cpu qemu64 -m 512M -smp 4 \
		-serial stdio \
		-bios /opt/homebrew/share/qemu/edk2-x86_64-code.fd \
		-drive file=$(IMG),format=raw \
		-no-reboot
endif

# Run with GUI window (shows framebuffer)
run-gui: image
ifeq ($(ARCH),aarch64)
	qemu-system-aarch64 \
		-M virt -cpu cortex-a72 -m 1G -smp 4 \
		-serial stdio \
		-bios /opt/homebrew/share/qemu/edk2-aarch64-code.fd \
		-drive file=$(IMG),format=raw \
		-device ramfb \
		-device qemu-xhci -device usb-kbd -device usb-mouse \
		-no-reboot
else
	qemu-system-x86_64 \
		-M q35 -cpu qemu64 -m 1G -smp 4 \
		-serial stdio \
		-bios /opt/homebrew/share/qemu/edk2-x86_64-code.fd \
		-drive file=$(IMG),format=raw \
		-device qemu-xhci -device usb-kbd -device usb-mouse \
		-no-reboot
endif

clean:
	cargo clean
	rm -rf target/*.img target/iso_root
