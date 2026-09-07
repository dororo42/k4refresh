//! einkfb 驱动封装：open / ioctl / fbinfo。
//!
//! Kindle 4（K2↔K4 一并适用）的 legacy einkfb 刷新 ABI，ioctl 号与结构体
//! 布局逐字段对齐 FBInk `eink/einkfb.h`（refresh_legacy() 即用同一对 ioctl），
//! 并与 KOReader `ffi/einkfb_h.lua` 的 cdef 交叉核对。
//!
//! unsafe 清单（全 crate 仅本文件内，理由见各注释）：
//! 1. open/close —— 单行 libc FFI，入参为合法 CStr / 已有 fd；
//! 2. ioctl 组 —— 按驱动 ABI 传指针/值，驱动只读我们的入参结构；
//! 3. bench 的 mmap —— 仅 CLI 使用，用 fix_info 的 smem_len 界定长度。

use std::ffi::CStr;
use std::io;
use std::os::raw::{c_int, c_ulong, c_void};

/// _IO('F', 0xdb)：整屏刷新，参数为 fx_type 值。
pub const FBIO_EINK_UPDATE_DISPLAY: u64 = 0x46db;
/// _IO('F', 0xdd)：区域刷新，参数为 update_area_t*。
pub const FBIO_EINK_UPDATE_DISPLAY_AREA: u64 = 0x46dd;
/// 读取 fb_var_screeninfo（面板分辨率/位深）。
pub const FBIOGET_VSCREENINFO: u64 = 0x4600;
/// 读取 fb_fix_screeninfo（framebuffer 物理布局，bench 用）。
pub const FBIOGET_FSCREENINFO: u64 = 0x4602;

/// ioctl 的 request 参数类型因 libc 实现而异（musl 是 int，glibc 是
/// unsigned long），用 cfg 包装抹平；`as _` 按包装签名自动转换。
#[cfg(target_env = "musl")]
unsafe fn ioctl_ptr(fd: c_int, request: u64, arg: *mut c_void) -> c_int {
    // unsafe 2/3：request 经 `as` 收窄为 musl 的 c_int；四个常量均 < 2^31，无损。
    libc::ioctl(fd, request as c_int, arg)
}
#[cfg(not(target_env = "musl"))]
unsafe fn ioctl_ptr(fd: c_int, request: u64, arg: *mut c_void) -> c_int {
    libc::ioctl(fd, request as c_ulong, arg)
}

/// 与 C `struct update_area_t` 逐字段一致（ARM32 上 24 字节）：
/// `{ int x1, y1, x2, y2; fx_type which_fx; uint8_t *buffer; }`
/// x2/y2 为右/下开边界（KOReader/FBInk 均按 x+w/y+h 传入）。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct UpdateArea {
    pub x1: i32,
    pub y1: i32,
    pub x2: i32,
    pub y2: i32,
    pub which_fx: i32,
    pub buffer: *mut c_void,
}

impl UpdateArea {
    pub fn new(x1: i32, y1: i32, x2: i32, y2: i32, which_fx: i32) -> Self {
        UpdateArea {
            x1,
            y1,
            x2,
            y2,
            which_fx,
            buffer: std::ptr::null_mut(),
        }
    }
}

/// fb_var_screeninfo 的"够用"镜像：只读前 7 个字段（分辨率/位深），
/// 尾部以 256 字节占位，保证缓冲区不小于任何 2.6.x~4.x 内核的 struct 大小。
#[repr(C)]
pub struct VarScreenInfo {
    pub xres: u32,
    pub yres: u32,
    pub xres_virtual: u32,
    pub yres_virtual: u32,
    pub xoffset: u32,
    pub yoffset: u32,
    pub bits_per_pixel: u32,
    pub _tail: [u8; 256],
}

impl VarScreenInfo {
    fn zeroed() -> Self {
        VarScreenInfo {
            xres: 0,
            yres: 0,
            xres_virtual: 0,
            yres_virtual: 0,
            xoffset: 0,
            yoffset: 0,
            bits_per_pixel: 0,
            _tail: [0u8; 256],
        }
    }
}

/// fb_fix_screeninfo 的"够用"镜像：bench 只需要 line_length / smem_len。
/// 字段顺序与 linux/fb.h 一致（ARM32 上 smem_start 为 4 字节 unsigned long）。
#[repr(C)]
pub struct FixScreenInfo {
    pub id: [u8; 16],
    pub smem_start: u32,
    pub smem_len: u32,
    pub fb_type: u32,
    pub type_aux: u32,
    pub visual: u32,
    pub xpanstep: u16,
    pub ypanstep: u16,
    pub ywrapstep: u16,
    pub line_length: u32,
    pub _tail: [u8; 256],
}

impl FixScreenInfo {
    fn zeroed() -> Self {
        FixScreenInfo {
            id: [0u8; 16],
            smem_start: 0,
            smem_len: 0,
            fb_type: 0,
            type_aux: 0,
            visual: 0,
            xpanstep: 0,
            ypanstep: 0,
            ywrapstep: 0,
            line_length: 0,
            _tail: [0u8; 256],
        }
    }
}

/// 打开 framebuffer 设备（默认 /dev/fb0），返回 fd。
/// unsafe 1/3：单行 libc FFI，path 为调用方保证以 NUL 结尾的合法 CStr。
pub fn open_fb(path: &CStr) -> io::Result<c_int> {
    let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDWR) };
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(fd)
    }
}

/// unsafe 1/3：单行 libc FFI，fd 由 open_fb 产生或调用方自持。
pub fn close_fd(fd: c_int) {
    unsafe {
        libc::close(fd);
    }
}

/// 区域刷新：发 FBIO_EINK_UPDATE_DISPLAY_AREA。
/// unsafe 2/3：按驱动 ABI 传 24 字节 update_area_t 指针；驱动只读入参，
/// 失败返回 -1 且置 errno，不做任何用户态内存写入。
pub fn update_area(fd: c_int, area: &UpdateArea) -> io::Result<()> {
    let mut a = *area;
    let rv = unsafe {
        ioctl_ptr(
            fd,
            FBIO_EINK_UPDATE_DISPLAY_AREA,
            &mut a as *mut UpdateArea as *mut c_void,
        )
    };
    if rv < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// 整屏刷新：发 FBIO_EINK_UPDATE_DISPLAY，参数为 fx 值（FBInk 整屏分支同款）。
pub fn update_display(fd: c_int, which_fx: i32) -> io::Result<()> {
    let rv = unsafe { ioctl_ptr(fd, FBIO_EINK_UPDATE_DISPLAY, which_fx as c_ulong as *mut c_void) };
    if rv < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// 读取面板信息（xres / yres / bits_per_pixel）。
pub fn var_info(fd: c_int) -> io::Result<VarScreenInfo> {
    let mut v = VarScreenInfo::zeroed();
    let rv = unsafe { ioctl_ptr(fd, FBIOGET_VSCREENINFO, &mut v as *mut VarScreenInfo as *mut c_void) };
    if rv < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(v)
    }
}

/// 读取 framebuffer 物理布局（bench 的 mmap 与逐行写内容需要）。
pub fn fix_info(fd: c_int) -> io::Result<FixScreenInfo> {
    let mut f = FixScreenInfo::zeroed();
    let rv = unsafe { ioctl_ptr(fd, FBIOGET_FSCREENINFO, &mut f as *mut FixScreenInfo as *mut c_void) };
    if rv < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(f)
    }
}
