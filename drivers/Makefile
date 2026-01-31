DRIVERS := acpi pci nvme ahci namespace rootns

all:
	for item in $(DRIVERS); do \
		$(MAKE) -C $$item || exit 1; \
	done

clippy:
	for item in $(DRIVERS); do \
		$(MAKE) -C $$item clippy || exit 1; \
	done

fmt:
	for item in $(DRIVERS); do \
		$(MAKE) -C $$item fmt || exit 1; \
	done
