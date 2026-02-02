SOURCE ?= $(shell pwd)

RUST_TARGET ?= $(ARCH)-unknown-none
ifeq ($(RUST_PROFILE), dev)
RUST_PROFILE_SUBDIR := debug
else
RUST_PROFILE_SUBDIR := release
endif

LOCKFILE := $(SOURCE)/Cargo.lock
MANIFEST := $(SOURCE)/Cargo.toml

export BUILD = $(SOURCE)/build

.PHONY: $(BUILD)/nvme-$(ARCH).elf
$(BUILD)/nvme-$(ARCH).elf:
	mkdir -p $(BUILD)
	RUSTFLAGS="-C relocation-model=static" cargo build --profile $(RUST_PROFILE) --target $(RUST_TARGET)
	cp target/$(RUST_TARGET)/$(RUST_PROFILE_SUBDIR)/nvmed $(BUILD)/nvme-$(ARCH).elf
	cp $(BUILD)/nvme-$(ARCH).elf $(BUILD)/nvme.elf

clippy:
	cargo clippy --target $(RUST_TARGET) -Zbuild-std=core,alloc

fmt:
	cargo fmt
