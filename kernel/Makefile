SOURCE ?= $(shell pwd)
LD_SCRIPT=$(SOURCE)/linkers/$(ARCH).ld

RUST_TARGET ?= $(ARCH)-unknown-none
ifeq ($(RUST_PROFILE), dev)
RUST_PROFILE_SUBDIR := debug
else
RUST_PROFILE_SUBDIR := release
endif

LOCKFILE := $(SOURCE)/Cargo.lock
MANIFEST := $(SOURCE)/Cargo.toml

export BUILD = $(SOURCE)/build

QEMUFLAGS ?= -m 4G -serial stdio
ifneq ($(DEBUG), 0)
QEMUFLAGS += -s -S
endif
ifneq ($(KVM), 0)
QEMUFLAGS += -cpu host --enable-kvm
else
QEMUFLAGS += -cpu max
endif

.PHONY: $(BUILD)/kernel-$(ARCH).elf
$(BUILD)/kernel-$(ARCH).elf:
	mkdir -p $(BUILD)
	RUSTFLAGS="-C relocation-model=static" cargo build --profile $(RUST_PROFILE) --target $(RUST_TARGET)
	cp target/$(RUST_TARGET)/$(RUST_PROFILE_SUBDIR)/radon_kernel $(BUILD)/kernel-$(ARCH).elf

clippy:
	cargo clippy --target $(RUST_TARGET) -Zbuild-std=core,alloc

fmt:
	cargo fmt

limine/limine:
	rm -rf limine
	git clone https://github.com/limine-bootloader/limine.git --branch=v10.x-binary --depth=1
	$(MAKE) -C limine

.PHONY: initramfs-$(ARCH).img
initramfs-$(ARCH).img: ../init/build/init-$(ARCH).elf
	rm -rf initramfs-$(ARCH).img
	mkdir -p initramfs-$(ARCH)/sbin
	cp ../init/build/init-$(ARCH).elf initramfs-$(ARCH)/sbin/init
	sh mkinitcpio.sh

.PHONY: aether-$(ARCH).img
aether-$(ARCH).img: limine/limine initramfs-$(ARCH).img $(BUILD)/kernel-$(ARCH).elf
	rm -f aether-$(ARCH).img
	dd if=/dev/zero bs=1M count=0 seek=256 of=aether-$(ARCH).img
	sgdisk aether-$(ARCH).img -n 1:2048 -t 1:ef00
	mformat -i aether-$(ARCH).img@@1M
	mmd -i aether-$(ARCH).img@@1M ::/EFI ::/EFI/BOOT ::/boot ::/boot/limine
	mcopy -i aether-$(ARCH).img@@1M $(BUILD)/kernel-$(ARCH).elf ::/boot/kernel.elf
	mcopy -i aether-$(ARCH).img@@1M limine.conf ::/boot/limine
	mcopy -i aether-$(ARCH).img@@1M initramfs-$(ARCH).img ::/initramfs.img
ifeq ($(ARCH),x86_64)
	mcopy -i aether-$(ARCH).img@@1M limine/limine-bios.sys ::/boot/limine
	mcopy -i aether-$(ARCH).img@@1M limine/BOOTX64.EFI ::/EFI/BOOT
	mcopy -i aether-$(ARCH).img@@1M limine/BOOTIA32.EFI ::/EFI/BOOT
endif
ifeq ($(ARCH),aarch64)
	mcopy -i aether-$(ARCH).img@@1M limine/BOOTAA64.EFI ::/EFI/BOOT
endif
ifeq ($(ARCH),riscv64)
	mcopy -i aether-$(ARCH).img@@1M limine/BOOTRISCV64.EFI ::/EFI/BOOT
endif
ifeq ($(ARCH),loongarch64)
	mcopy -i aether-$(ARCH).img@@1M limine/BOOTLOONGARCH64.EFI ::/EFI/BOOT
endif

.PHONY: rootfs-$(ARCH).img
rootfs-$(ARCH).img:
	mkdir -p rootfs-$(ARCH)
	dd if=/dev/zero bs=1M count=0 seek=1024 of=rootfs-$(ARCH).img
	mkfs.ext2 -b 1024 -F -q -d rootfs-$(ARCH) rootfs-$(ARCH).img

edk2-ovmf:
	curl -L https://github.com/osdev0/edk2-ovmf-nightly/releases/download/nightly-20260129T015230Z/edk2-ovmf.tar.gz | gunzip | tar -xf -

run: run-$(ARCH)

run-x86_64: edk2-ovmf aether-$(ARCH).img rootfs-$(ARCH).img
	qemu-system-x86_64 \
		-M q35 \
		-smp $(SMP) \
		-device qemu-xhci,id=xhci \
		-device usb-kbd \
		-device usb-mouse \
		-device ahci,id=ahci \
		-drive if=pflash,unit=0,format=raw,file=edk2-ovmf/ovmf-code-$(ARCH).fd,readonly=on \
		-drive if=none,file=aether-$(ARCH).img,format=raw,id=hdd \
		-device nvme,drive=hdd,serial=hdd \
		-drive if=none,file=rootfs-$(ARCH).img,format=raw,id=root \
		-device nvme,drive=root,serial=root \
		$(QEMUFLAGS)

run-aarch64: edk2-ovmf aether-$(ARCH).img rootfs-$(ARCH).img
	qemu-system-aarch64 \
		-M virt \
		-smp $(SMP) \
		-cpu cortex-a72 \
		-device ramfb \
		-device qemu-xhci,id=xhci \
		-device usb-kbd \
		-device usb-mouse \
		-drive if=pflash,unit=0,format=raw,file=edk2-ovmf/ovmf-code-$(ARCH).fd,readonly=on \
		-drive if=none,file=aether-$(ARCH).img,format=raw,id=hdd \
		-device nvme,drive=hdd,serial=hdd \
		-drive if=none,file=rootfs-$(ARCH).img,format=raw,id=root \
		-device nvme,drive=root,serial=root \
		$(QEMUFLAGS)
