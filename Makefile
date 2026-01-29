export ARCH ?= x86_64
export RUST_PROFILE ?= dev

export DEBUG ?= 0
export SMP ?= 2

all:
	$(MAKE) -C nameserver
	$(MAKE) -C drivers
	$(MAKE) -C init
	$(MAKE) -C kernel

clippy:
	$(MAKE) -C nameserver clippy
	$(MAKE) -C drivers clippy
	$(MAKE) -C init clippy
	$(MAKE) -C kernel clippy

fmt:
	$(MAKE) -C nameserver fmt
	$(MAKE) -C drivers fmt
	$(MAKE) -C init fmt
	$(MAKE) -C kernel fmt

run:
	$(MAKE) -C nameserver
	$(MAKE) -C drivers
	$(MAKE) -C init
	$(MAKE) -C kernel run
