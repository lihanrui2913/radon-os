cd initramfs-${ARCH}
find . -print | cpio -o -H newc >../initramfs-${ARCH}.img
cd ..
