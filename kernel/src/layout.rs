/// 用户空间起始地址
pub const USER_SPACE_START: usize = 0x0000_0000_0000_1000;
/// 用户空间结束地址
pub const USER_SPACE_END: usize = 0x0000_7FFF_FFFF_0000;
/// 默认栈大小 (8 MB)
pub const DEFAULT_STACK_SIZE: usize = 32 * 1024 * 1024;
/// 栈顶地址
pub const STACK_TOP: usize = 0x0000_7FFF_FFFF_0000;
/// 堆起始地址（动态确定）
pub const ALLOC_START: usize = 0x0000_6000_0000_0000;
