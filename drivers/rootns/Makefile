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

.PHONY: $(BUILD)/rootns-$(ARCH).elf
$(BUILD)/rootns-$(ARCH).elf:
	mkdir -p $(BUILD)
	RUSTFLAGS="-C relocation-model=static" cargo build --profile $(RUST_PROFILE) --target $(RUST_TARGET)
	cp target/$(RUST_TARGET)/$(RUST_PROFILE_SUBDIR)/rootns $(BUILD)/rootns-$(ARCH).elf
	cp $(BUILD)/rootns-$(ARCH).elf $(BUILD)/rootns.elf

clippy:
	cargo clippy --target $(RUST_TARGET) -Zbuild-std=core,alloc

fmt:
	cargo fmt
