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

.PHONY: $(BUILD)/pci-$(ARCH).elf
$(BUILD)/pci-$(ARCH).elf:
	mkdir -p $(BUILD)
	RUSTFLAGS="-C relocation-model=static" cargo build --profile $(RUST_PROFILE) --target $(RUST_TARGET) -Zbuild-std-features=compiler-builtins-mem
	cp target/$(RUST_TARGET)/$(RUST_PROFILE_SUBDIR)/pcid $(BUILD)/pci-$(ARCH).elf
	cp $(BUILD)/pci-$(ARCH).elf $(BUILD)/pci.elf

clippy:
	cargo clippy --target $(RUST_TARGET) -Zbuild-std=core,alloc

fmt:
	cargo fmt
