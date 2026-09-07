-- K4Refresh.lua — KOReader 端最小桥接（方案文档 §6 步骤 3 的载体）
--
-- 作用：把 libk4refresh.so 暴露成 KOReader 内可调用的刷新接口。
-- 这是最小可用版本：手动模式（KK code 触发全刷 / 菜单脚本设模式），
-- 不自动接管 refreshPartial/refreshFull —— 自动接管属于第二阶段，
-- 需要按 KOReader nightly 的 einkfb 后端接口重新对齐（文档 §8 风险 R4）。
--
-- 安装位置（二选一，文档 §6）：
--   A. koreader/settings/ 旁的自定义目录（推荐，免碰 KOReader 本体）
--   B. koreader/frontend/（需随 KOReader 升级重放，不推荐）
--
-- 用法（KOReader 内置 Lua 控制台或挂在菜单/手势上）：
--   local K4R = require("K4Refresh")   -- 路径需在 package.path 中
--   K4R.init()                         -- 幂等
--   K4R.flash()                        -- 立即 fx_update_slow 整屏
--   K4R.set_mode(0, 6)                 -- fast 模式，每 6 页 slow 收尾
--   K4R.set_mode(1)                    -- 回 conservative（等价原生行为）
--   K4R.close()

local logger = require("logger")

local K4R = {
    loaded = false,
    fd = -1,
    ok = false,
}

local function library_candidates()
    -- 按部署位置依次探测（文档 §6 步骤 2 的两个安装点）
    return {
        "k4refresh/libk4refresh.so",       -- /mnt/us/k4refresh/
        "/mnt/us/k4refresh/libk4refresh.so",
        "libk4refresh.so",                  -- 系统 ldpath 兜底
    }
end

function K4R.init()
    if K4R.loaded then return K4R.ok end
    K4R.loaded = true
    local ffi_ok, ffi = pcall(require, "ffi")
    if not ffi_ok then
        logger.warn("K4Refresh: LuaJIT FFI 不可用，插件停用")
        return false
    end
    -- C ABI 与 src/lib.rs 的 #[no_mangle] 导出一一对应
    ffi.cdef[[
        int  k4refresh_open(const char *path);
        int  k4refresh_refresh(int fd, unsigned int kind, int x1, int y1, int x2, int y2);
        int  k4refresh_flash(int fd);
        void k4refresh_set_mode(unsigned int mode, unsigned int interval);
        int  k4refresh_screen_size(int fd, int *w_out, int *h_out);
        int  k4refresh_last_error(char *buf, int len);
        void k4refresh_close(int fd);
    ]]
    local lib = nil
    for _, name in ipairs(library_candidates()) do
        local ok, handle = pcall(ffi.load, name)
        if ok then lib = handle; break end
    end
    if not lib then
        logger.warn("K4Refresh: libk4refresh.so 未找到（见方案文档 §6 部署步骤）")
        return false
    end
    K4R.lib = lib
    local fd = lib.k4refresh_open("/dev/fb0")
    if fd < 0 then
        logger.warn("K4Refresh: 打开 /dev/fb0 失败")
        return false
    end
    K4R.fd = fd
    K4R.ok = true
    logger.info("K4Refresh: 已加载，fd =", fd)
    return true
end

-- kind: 0=翻页/局部 1=UI 2=显式全刷（与 fx.rs KIND_* 一致）
function K4R.refresh(kind, x1, y1, x2, y2)
    if not K4R.ok then return nil end
    return K4R.lib.k4refresh_refresh(K4R.fd, kind or 0,
        x1 or 0, y1 or 0, x2 or 0, y2 or 0)
end

function K4R.flash()
    if not K4R.ok then return nil end
    return K4R.lib.k4refresh_flash(K4R.fd)
end

-- mode: 0=fast 1=conservative；interval: N 次翻页后 slow 收尾
function K4R.set_mode(mode, interval)
    if not K4R.ok then return nil end
    K4R.lib.k4refresh_set_mode(mode or 0, interval or 6)
    return true
end

function K4R.screen_size()
    if not K4R.ok then return nil end
    local ffi = require("ffi")
    local w = ffi.new("int[1]")
    local h = ffi.new("int[1]")
    if K4R.lib.k4refresh_screen_size(K4R.fd, w, h) == 0 then
        return tonumber(w[0]), tonumber(h[0])
    end
    return nil
end

function K4R.close()
    if K4R.ok and K4R.fd >= 0 then
        K4R.lib.k4refresh_close(K4R.fd)
    end
    K4R.ok = false
    K4R.fd = -1
end

-- KOReader 卸载/休眠兜底：UIManager 生命周期外手动 close 由集成方负责
return K4R
