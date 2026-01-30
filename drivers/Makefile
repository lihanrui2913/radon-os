all:
	$(MAKE) -C acpi
	$(MAKE) -C pci
	$(MAKE) -C nvme
	$(MAKE) -C ahci

clippy:
	$(MAKE) -C acpi clippy
	$(MAKE) -C pci clippy
	$(MAKE) -C nvme clippy
	$(MAKE) -C ahci clippy

fmt:
	$(MAKE) -C acpi fmt
	$(MAKE) -C pci fmt
	$(MAKE) -C nvme fmt
	$(MAKE) -C ahci fmt
