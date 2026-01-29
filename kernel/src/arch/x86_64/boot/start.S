.global _kernel_start
_kernel_start:
    jmp kmain

.global _ap_start
_ap_start:
    jmp ap_kmain
