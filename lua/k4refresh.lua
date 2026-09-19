-- k4refresh.lua — KOReader 端最小桥接（手动/控制台用）
--
-- v0.1.4 起自动化的主路径是 k4refresh.koplugin 插件（翻页计数收尾），
-- 本桥保留给 Lua 调试台手动调用与接口验证。两者共享 libk4refresh.so。
-- 这是最小可用版本：手动模式（KK code 触发全刷 / 菜单脚本设模式），
-- 不自动接管 refreshPartial/refreshFull —— 自动接管见插件（技术方案 O-1 v2）。
--
-- 安装位置（二选一，文档 §6）：
--   A. koreader/settings/ 旁的自定义目录（推荐，免碰 KOReader 本体）
--   B. koreader/frontend/（需随 KOReader 升级重放，不推荐）
--
-- 用法（KOReader 内置 Lua 控制台或挂在菜单/手势上）：
--   local K4R = require("k4refresh")   -- 文件名小写，路径需在 package.path 中
--   K4R.init()                         -- 幂等；init 时自动读取 KUAL 模式文件
--   K4R.flash()                        -- 立即 fx_update_slow 整屏
--   K4R.set_mode(0, 6)                 -- fast 模式，每 6 页 slow 收尾
--   K4R.set_mode(1)                    -- 回 conservative（等价原生行为）
--   K4R.close()
--
-- 模式文件（KUAL 菜单 Set Fast (6)/Set Conservative 写入，跨进程生效通道）：
--   /mnt/us/k4refresh/mode.conf        内容: "fast N" 或 "conservative"

local logger = require("logger")

local MODE_CONF_CANDIDATES = {
    "/mnt/us/k4refresh/mode.conf",
    "k4refresh/mode.conf",
}

-- FFI 路径不可用时的兜底：静态 CLI（零依赖，任何 ABI 的宿主进程都能 spawn）。
-- 真机 2026-09-19 证实：K4 用户态为 armel/softfp（/lib/ld-linux.so.3）+ glibc
-- 2.12.1，而 armhf/glibc≥2.18 基线的 libk4refresh.so 无法 dlopen——回退路径
-- 是当前设备上唯一可用的 KOReader 内入口（插件同款逻辑）。
local CLI = "/mnt/us/k4refresh/k4refresh-cli"

local K4R = {
    loaded = false,
    ok = false,
    fd = -1,
    via_cli = false,
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

    local lib = nil
    local ffi_ok, ffi = pcall(require, "ffi")
    if ffi_ok then
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
        for _, name in ipairs(library_candidates()) do
            local ok, handle = pcall(ffi.load, name)
            if ok then lib = handle; break end
        end
        if lib then
            local fd = lib.k4refresh_open("/dev/fb0")
            if fd >= 0 then
                K4R.lib = lib
                K4R.fd = fd
                K4R.via_cli = false
                K4R.ok = true
                logger.info("K4Refresh: FFI 已加载，fd =", fd)
            else
                logger.warn("K4Refresh: 打开 /dev/fb0 失败，回退静态 CLI")
            end
        else
            logger.warn("K4Refresh: libk4refresh.so dlopen 失败（armel 设备 + armhf 产物，见已知边界），回退静态 CLI")
        end
    else
        logger.warn("K4Refresh: LuaJIT FFI 不可用，回退静态 CLI")
    end

    if not K4R.ok then
        -- CLI 兜底：只要求二进制存在且可执行
        local f = io.open(CLI, "r")
        if f then
            f:close()
            K4R.via_cli = true
            K4R.ok = true
            logger.info("K4Refresh: via=cli（静态 CLI 兜底）")
        else
            logger.warn("K4Refresh: CLI 兜底也不可用（", CLI, " 不存在），停用")
            return false
        end
    end

    K4R.load_mode_from_file()   -- 应用 KUAL「Ghost 档位」写入的模式文件
    return true
end

-- 读取 KUAL/插件写入的模式文件并应用。返回 true 表示已应用，否则 nil。
-- 文件格式: "ghost N"/"fast N"（fast 为 v0.1.x 旧档等价迁移）或
-- "conservative"/"off"；内容非法则静默忽略。
function K4R.load_mode_from_file()
    if not K4R.ok then return nil end
    for _, path in ipairs(MODE_CONF_CANDIDATES) do
        local f = io.open(path, "r")
        if f then
            local line = f:read("*l") or ""
            f:close()
            local mode = line:match("^(%S+)")
            local n = tonumber(line:match("^%S+%s+(%S+)"))
            if mode == "ghost" or mode == "fast" then
                K4R.set_mode(0, n or 6)
                logger.info("K4Refresh: mode.conf -> fast interval =", n or 6)
                return true
            elseif mode == "conservative" or mode == "off" then
                K4R.set_mode(1, 6)
                logger.info("K4Refresh: mode.conf -> conservative/off")
                return true
            end
        end
    end
    return nil
end

-- kind: 0=翻页/局部 1=UI 2=显式全刷（与 fx.rs KIND_* 一致）
-- via=cli 时经静态 CLI 一次性执行（区域参数仅 FFI 路径支持）
function K4R.refresh(kind, x1, y1, x2, y2)
    if not K4R.ok then return nil end
    if K4R.via_cli then
        local k = ({ [0] = "page", [1] = "ui", [2] = "full" })[kind or 0] or "page"
        if k == "full" then
            return K4R.flash()
        end
        os.execute(CLI .. " refresh --kind " .. k .. " >/dev/null 2>&1")
        return true
    end
    return K4R.lib.k4refresh_refresh(K4R.fd, kind or 0,
        x1 or 0, y1 or 0, x2 or 0, y2 or 0)
end

function K4R.flash()
    if not K4R.ok then return nil end
    if K4R.via_cli then
        os.execute(CLI .. " flash >/dev/null 2>&1")
        return true
    end
    return K4R.lib.k4refresh_flash(K4R.fd)
end

-- mode: 0=fast 1=conservative；interval: N 次翻页后 slow 收尾
-- 注：库内模式无翻页路径消费者（见已知边界），跨进程档位以 mode.conf
-- + 插件为准；此接口保留为 ABI 兼容，via=cli 时仅记录。
function K4R.set_mode(mode, interval)
    if not K4R.ok then return nil end
    if K4R.via_cli then
        logger.info("K4Refresh: set_mode 忽略（via=cli，档位由 mode.conf/插件管理）")
        return true
    end
    K4R.lib.k4refresh_set_mode(mode or 0, interval or 6)
    return true
end

function K4R.screen_size()
    if not K4R.ok then return nil end
    if K4R.via_cli then return nil end
    local ffi = require("ffi")
    local w = ffi.new("int[1]")
    local h = ffi.new("int[1]")
    if K4R.lib.k4refresh_screen_size(K4R.fd, w, h) == 0 then
        return tonumber(w[0]), tonumber(h[0])
    end
    return nil
end

function K4R.close()
    if K4R.ok and not K4R.via_cli and K4R.fd >= 0 then
        K4R.lib.k4refresh_close(K4R.fd)
    end
    K4R.ok = false
    K4R.fd = -1
end

-- KOReader 卸载/休眠兜底：UIManager 生命周期外手动 close 由集成方负责
return K4R
