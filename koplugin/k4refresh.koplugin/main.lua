-- k4refresh.koplugin — Kindle 4 重影优先模式（v0.1.4 / 技术方案 O-1 v2）
--
-- 职责：接管"翻页清残影"的调度，不接管翻页本身——
--   翻页仍走 KOReader 原生 partial（真机 2026-09-08 实测：真实翻页下
--   fx partial/fast/slow 视觉不可分辨，"fast 档加速翻页"前提不成立，
--   见 .workbuddy 长期笔记），本插件只做计数收尾：
--   - 每 N 次翻页 → libk4refresh 的 slow 全刷一次（清残影，闪烁一次）；
--   - 大步长跳转（章节/跳页，|Δpage|>1）→ 立即 slow 全刷；
--   - interval 经 /mnt/us/k4refresh/mode.conf 与 KUAL/CLI 互通。
--
-- 兜底：FFI 不可用 / libk4refresh.so 加载失败 / open 失败时，自动改用
-- 静态 CLI（os.execute），保证收尾动作永远可达。
--
-- 安装：/mnt/us/koreader/plugins/k4refresh.koplugin/{_meta.lua, main.lua}
--   （libk4refresh.so 与 k4refresh-cli 仍在 /mnt/us/k4refresh/，部署同 README §3）

local logger = require("logger")
local WidgetContainer = require("ui/widget/container/widgetcontainer")

local MODE_CONF = "/mnt/us/k4refresh/mode.conf"
local CLI_FLASH = "/mnt/us/k4refresh/k4refresh-cli flash >/dev/null 2>&1"

local K4R = WidgetContainer:extend{
    name = "k4refresh",
    is_doc_only = true,
}

function K4R:init()
    self.interval = nil      -- nil=off；N=每 N 页一次 slow 收尾
    self.flips = 0
    self.last_pageno = nil
    self.lib = nil
    self.fd = -1
    self.via_cli = true      -- loadLib 成功则翻转为 false
    self:loadLib()
    self:loadConf()
    if self.ui.menu then
        self.ui.menu:registerToMainMenu(self)
    end
    logger.info(string.format("K4Refresh: plugin ready (interval=%s, via=%s)",
        tostring(self.interval or "off"), self.via_cli and "cli" or "ffi"))
end

-- FFI 装载：绝对路径优先（KOReader 运行目录不保证），失败退回 CLI 路径
function K4R:loadLib()
    local ffi_ok, ffi = pcall(require, "ffi")
    if not ffi_ok then
        logger.warn("K4Refresh: LuaJIT FFI 不可用，回退静态 CLI")
        return
    end
    -- C ABI 与 src/lib.rs 的 #[no_mangle] 导出一一对应（v0.1.x 冻结，7 个函数）
    ffi.cdef[[
        int  k4refresh_open(const char *path);
        int  k4refresh_refresh(int fd, unsigned int kind, int x1, int y1, int x2, int y2);
        int  k4refresh_flash(int fd);
        void k4refresh_set_mode(unsigned int mode, unsigned int interval);
        int  k4refresh_screen_size(int fd, int *w_out, int *h_out);
        int  k4refresh_last_error(char *buf, int len);
        void k4refresh_close(int fd);
    ]]
    for _, name in ipairs({ "/mnt/us/k4refresh/libk4refresh.so", "libk4refresh.so" }) do
        local ok, handle = pcall(ffi.load, name)
        if ok then self.lib = handle; break end
    end
    if not self.lib then
        logger.warn("K4Refresh: libk4refresh.so 加载失败，回退静态 CLI")
        return
    end
    self.fd = self.lib.k4refresh_open("/dev/fb0")
    if self.fd < 0 then
        logger.warn("K4Refresh: 打开 /dev/fb0 失败，回退静态 CLI")
        self.lib = nil
        return
    end
    self.via_cli = false
end

-- slow 全刷收尾。FFI 路径失败不重试（下个计数周期自然再试）；CLI 路径静默。
function K4R:flash()
    if self.lib then
        self.lib.k4refresh_flash(self.fd)
    else
        os.execute(CLI_FLASH)
    end
end

-- 解析 mode.conf（KUAL 写入 / 插件菜单写入）：
--   "ghost N"  → 每 N 页收尾（v0.1.4 主格式）
--   "fast N"   → 旧版 v0.1.x 档位，等价迁移为每 N 页收尾
--   "off" / "conservative" / 其他 → 关闭
function K4R:loadConf()
    local prev = self.interval
    self.interval = nil
    for _, path in ipairs({ MODE_CONF, "k4refresh/mode.conf" }) do
        local f = io.open(path, "r")
        if f then
            local line = f:read("*l") or ""
            f:close()
            local mode = line:match("^(%S+)")
            local n = tonumber(line:match("^%S+%s+(%S+)"))
            if (mode == "ghost" or mode == "fast") and n and n >= 1 then
                self.interval = math.min(99, math.floor(n))
            end
            break
        end
    end
    if self.interval ~= prev then
        self.flips = 0
    end
    logger.info(string.format("K4Refresh: mode.conf -> interval=%s",
        tostring(self.interval or "off")))
end

-- 菜单/初始化路径统一走这里：改状态并落盘 mode.conf，KUAL/CLI 侧保持一致
function K4R:setInterval(n)
    self.interval = n
    self.flips = 0
    self:takeover_builtin_refresh()
    os.execute("mkdir -p /mnt/us/k4refresh 2>/dev/null")
    local f = io.open(MODE_CONF, "w")
    if f then
        if n then
            f:write(string.format("ghost %d\n", n))
        else
            f:write("off\n")
        end
        f:close()
    end
end

function K4R:onReaderReady()
    -- 每次开书重读 mode.conf：KUAL 改档无需重启 KOReader
    self:loadConf()
    self.flips = 0
    self.last_pageno = nil
    self:takeover_builtin_refresh()
end

-- 收尾策略接管：ghost 模式激活时关闭 KOReader 内置的每 N 页 promotion 全刷
-- （UIManager 默认 FULL_REFRESH_COUNT=6，会与插件收尾叠加成双黑闪）；
-- ghost 关闭时恢复默认 6。setRefreshRate 会持久化 full_refresh_count。
function K4R:takeover_builtin_refresh()
    pcall(function()
        local UIManager = require("ui/uimanager")
        if not UIManager.setRefreshRate then
            logger.warn("K4Refresh: UIManager.setRefreshRate 不可用，跳过内置全刷接管")
            return
        end
        if self.interval then
            UIManager:setRefreshRate(0)
            logger.info("K4Refresh: KOReader builtin full-refresh disabled (plugin owns policy)")
        else
            UIManager:setRefreshRate(6)
            logger.info("K4Refresh: ghost off -> KOReader builtin full-refresh restored to 6")
        end
    end)
end

function K4R:onPageUpdate(pageno)
    if self.last_pageno == nil or pageno == self.last_pageno then
        self.last_pageno = pageno
        return
    end
    local delta = math.abs(pageno - self.last_pageno)
    self.last_pageno = pageno
    if not self.interval then
        return
    end
    if delta > 1 then
        -- 大步长跳转（章节/跳页/书签）：内容全换，直接收尾
        self.flips = 0
        self:flash()
        return
    end
    self.flips = self.flips + 1
    if self.flips >= self.interval then
        self.flips = 0
        self:flash()
    end
end

function K4R:onCloseDocument()
    self.flips = 0
    self.last_pageno = nil
end

function K4R:addToMainMenu(menu_items)
    menu_items.k4refresh = {
        text = "K4Refresh (ghost clearing)",
        sub_item_table = {
            {
                text = "Full refresh now (slow)",
                keep_menu_open = false,
                callback = function() self:flash() end,
            },
            {
                text = "Ghost clearing: Off",
                checked_func = function() return self.interval == nil end,
                callback = function() self:setInterval(nil) end,
            },
            {
                text = "Every 4 pages",
                checked_func = function() return self.interval == 4 end,
                callback = function() self:setInterval(4) end,
            },
            {
                text = "Every 6 pages",
                checked_func = function() return self.interval == 6 end,
                callback = function() self:setInterval(6) end,
            },
            {
                text = "Every 8 pages",
                checked_func = function() return self.interval == 8 end,
                callback = function() self:setInterval(8) end,
            },
        },
    }
end

return K4R
