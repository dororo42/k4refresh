-- k4refresh.koplugin — Kindle 4 重影优先模式（v0.1.10 / O-1 v2 + Duokan 量化）
--
-- 职责：接管"翻页清残影"的调度，不接管翻页本身——
--   翻页仍走 KOReader 原生 partial（真机 2026-09-08 实测：真实翻页下
--   fx partial/fast/slow 视觉不可分辨，"fast 档加速翻页"前提不成立，
--   见 .workbuddy 长期笔记），本插件只做计数收尾：
--   - 每 N 次翻页 → libk4refresh 的 slow 全刷一次（清残影，闪烁一次）；
--   - 大步长跳转（章节/跳页，|Δpage|>1）→ 立即 slow 全刷；
--   - interval 经 /mnt/us/k4refresh/mode.conf 与 KUAL/CLI 互通。
--
-- v0.1.10 新增「Duokan 量化」（逆向依据见
-- ~/reports/Duokan-xKindle-K4固件逆向与借鉴优化报告-2026-11-07.md）：
--   多看 KindleApp 在渲染层对每像素执行 v&0xF0|v>>4（16 级灰阶，幂等），
--   把 partial 刷新的累积灰阶污染锁死在 16 级内。本插件在 KOReader 的
--   refreshPartialImp/refreshFullImp（blit 后、ioctl 前的天然接管点，
--   ffi/framebuffer_einkfb.lua）之前对可见 fb 就地量化——纯 LuaJIT FFI
--   直写 KOReader 已 mmap 的 fb 内存，无需 libk4refresh.so，与方案 A 的
--   FFI 停用（armel 边界）无关。
--
-- 兜底（v0.1.8 方案 A）：K4 用户态 armel/glibc 2.12.1，gnueabihf 的
-- libk4refresh.so 必然 dlopen 失败（已知边界，readme §7），插件默认直接走
-- 静态 CLI（os.execute），收尾动作永远可达；仅当 /mnt/us/k4refresh/ffi.enabled
-- 标记存在（方案 B armel .so 落地后）才恢复 FFI 探测。
--
-- 安装：/mnt/us/koreader/plugins/k4refresh.koplugin/{_meta.lua, main.lua}
--   （libk4refresh.so 与 k4refresh-cli 仍在 /mnt/us/k4refresh/，部署同 README §3）

local logger = require("logger")
local WidgetContainer = require("ui/widget/container/widgetcontainer")

local MODE_CONF = "/mnt/us/k4refresh/mode.conf"
local CLI_FLASH = "/mnt/us/k4refresh/k4refresh-cli flash >/dev/null 2>&1"
-- FFI 探测总开关：K4 上默认停用（armel 设备无可加载的 .so）；方案 B 重建
-- armel .so 后 touch 该标记文件即恢复 FFI 路径，无需改代码。
local FFI_ENABLE = "/mnt/us/k4refresh/ffi.enabled"

local K4R = WidgetContainer:extend{
    name = "k4refresh",
    is_doc_only = true,
}

function K4R:init()
    self.interval = nil      -- nil=off；N=每 N 页一次 slow 收尾
    self.quant = false       -- Duokan 量化开关（v0.1.10）
    self._quant_installed = false
    self._orig_rp = nil      -- 接管前的 refreshPartialImp（还原用）
    self._orig_rf = nil
    self.flips = 0
    self.last_pageno = nil
    self.lib = nil
    self.fd = -1
    self.builtin_rate = nil  -- ghost 接管前的 KOReader 内置全刷档（还原用）
    self.via_cli = true      -- loadLib 成功则翻转为 false
    self:loadLib()
    self:loadConf()
    if self.ui.menu then
        self.ui.menu:registerToMainMenu(self)
    end
    logger.info(string.format("K4Refresh: plugin ready (interval=%s, quant=%s, via=%s)",
        tostring(self.interval or "off"), tostring(self.quant),
        self.via_cli and "cli" or "ffi"))
end

-- FFI 装载（v0.1.8 方案 A）：默认停用（K4 armel 无可加载 .so，readme §7），
-- 仅 ffi.enabled 标记存在时才探测；探测失败仍回退静态 CLI。
function K4R:loadLib()
    local mf = io.open(FFI_ENABLE, "r")
    if not mf then
        logger.info("K4Refresh: FFI 停用（方案 A: K4 armel 边界），走静态 CLI")
        return
    end
    mf:close()
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
--   第 1 行 "ghost N" → 每 N 页收尾（v0.1.4 主格式）
--           "fast N"  → 旧版 v0.1.x 档位，等价迁移为每 N 页收尾
--           "off" / "conservative" / 其他 → 关闭
--   第 2 行 "quant on|off" → Duokan 量化开关（v0.1.10，可缺省=off）
function K4R:loadConf()
    local prev_interval, prev_quant = self.interval, self.quant
    self.interval = nil
    for _, path in ipairs({ MODE_CONF, "k4refresh/mode.conf" }) do
        local f = io.open(path, "r")
        if f then
            local line1 = f:read("*l") or ""
            local line2 = f:read("*l") or ""
            f:close()
            local mode = line1:match("^(%S+)")
            local n = tonumber(line1:match("^%S+%s+(%S+)"))
            if (mode == "ghost" or mode == "fast") and n and n >= 1 then
                self.interval = math.min(99, math.floor(n))
            end
            if line2:match("^quant%s+on%s*$") then
                self.quant = true
            elseif line2:match("^quant%s+off%s*$") then
                self.quant = false
            end
            break
        end
    end
    if self.interval ~= prev_interval then
        self.flips = 0
    end
    if self.quant ~= prev_quant then
        self:applyQuantHook()
    end
    logger.info(string.format("K4Refresh: mode.conf -> interval=%s quant=%s",
        tostring(self.interval or "off"), tostring(self.quant)))
end

-- 菜单/初始化路径统一走这里：改状态并落盘 mode.conf，KUAL/CLI 侧保持一致。
-- 写 mode.conf 时保留第 2 行的 quant 开关（KUAL/插件双通道共用该文件）。
function K4R:setInterval(n)
    self.interval = n
    self.flips = 0
    self:takeover_builtin_refresh()
    os.execute("mkdir -p /mnt/us/k4refresh 2>/dev/null")
    local prev = self:readConfFile()
    local f = io.open(MODE_CONF, "w")
    if f then
        if n then
            f:write(string.format("ghost %d\n", n))
        else
            f:write("off\n")
        end
        f:write(string.format("quant %s\n", self.quant and "on" or "off"))
        f:close()
    end
end

-- 读 mode.conf 的第 1 行（保留 ghost 档位用；无文件返回 "off"）。
function K4R:readConfFile()
    local f = io.open(MODE_CONF, "r")
    if f then
        local line = f:read("*l") or "off"
        f:close()
        if line:match("%S") then return line end
    end
    return "off"
end

-- Duokan 量化开关：改状态 + 落盘第 2 行 + 安装/卸载刷新钩子（即时生效）。
function K4R:setQuant(on)
    self.quant = on and true or false
    self:applyQuantHook()
    os.execute("mkdir -p /mnt/us/k4refresh 2>/dev/null")
    local line1 = self:readConfFile()
    local f = io.open(MODE_CONF, "w")
    if f then
        f:write(line1 .. "\n")
        f:write(string.format("quant %s\n", self.quant and "on" or "off"))
        f:close()
    end
end

-- 可见 fb 就地 16 级量化（Duokan 同款 v&0xF0|v>>4，幂等）。
-- 纯 LuaJIT FFI：KOReader 的 bitbuffer 直接包着 mmap 的 fb 内存
--（ffi/framebuffer_linux.lua reinit：BB.new(..., self.data, finfo.line_length)），
-- 因此无需 libk4refresh.so，不与方案 A 的 FFI 停用冲突。
-- 物理尺寸/stride 取 _vinfo/_finfo（driver 实报值），只动可见区不动 pad。
-- 任一前提不满足（pcall 失败/非 8bpp）→ 返回 false，刷新照常。
function K4R:quantizeScreen()
    local ok, res = pcall(function()
        local Device = require("device")
        local fb = Device and Device.screen
        if not fb or not fb.data or not fb._vinfo or not fb._finfo then
            return false
        end
        if tonumber(fb._vinfo.bits_per_pixel) ~= 8 then
            return false
        end
        local ffi = require("ffi")
        local bit = require("bit")
        local w = tonumber(fb._vinfo.xres)
        local h = tonumber(fb._vinfo.yres)
        local stride = tonumber(fb._finfo.line_length)
        local p = ffi.cast("uint8_t*", fb.data)
        local band, rshift = bit.band, bit.rshift
        for y = 0, h - 1 do
            local row = p + y * stride
            for x = 0, w - 1 do
                local v = row[x]
                row[x] = band(v, 0xF0) + rshift(v, 4)
            end
        end
        return true
    end)
    if not ok then
        logger.warn("K4Refresh: quantizeScreen 异常（本次跳过）:", tostring(res))
        return false
    end
    return res == true
end

-- 量化钩子：接管 KOReader einkfb 后端的两个 Imp（blit 后、ioctl 前的唯一切面；
-- base 类的 refreshUI/Flash*/A2 等全部回落到这两个 Imp，包两处即全覆盖）。
-- 幂等：首次接管捕获原函数存 self._orig_*；重复调用只按当前开关切换指向。
function K4R:applyQuantHook()
    pcall(function()
        local Device = require("device")
        local screen = Device and Device.screen
        if not screen or not screen.refreshPartialImp then
            logger.warn("K4Refresh: screen.refreshPartialImp 不可用，量化钩子未安装")
            return
        end
        if not self._quant_installed then
            self._orig_rp = screen.refreshPartialImp
            self._orig_rf = screen.refreshFullImp
            self._quant_installed = true
        end
        if self.quant and self._orig_rp then
            screen.refreshPartialImp = function(s, x, y, w, h, d)
                K4R:quantizeScreen()
                return K4R._orig_rp(s, x, y, w, h, d)
            end
            screen.refreshFullImp = function(s, x, y, w, h, d)
                K4R:quantizeScreen()
                return K4R._orig_rf(s, x, y, w, h, d)
            end
            logger.info("K4Refresh: Duokan quant hook installed (16-level, idempotent)")
        elseif self._orig_rp then
            screen.refreshPartialImp = self._orig_rp
            screen.refreshFullImp = self._orig_rf
            logger.info("K4Refresh: Duokan quant hook removed")
        end
    end)
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
-- ghost 关闭时还原接管前档位（v0.1.7 硬编码 6，会覆盖用户自定义档；
-- v0.1.8 起首次接管捕获原值，持久化值即时回写，避免 0 落盘）。
function K4R:takeover_builtin_refresh()
    pcall(function()
        local UIManager = require("ui/uimanager")
        if not UIManager.setRefreshRate then
            logger.warn("K4Refresh: UIManager.setRefreshRate 不可用，跳过内置全刷接管")
            return
        end
        if self.interval then
            if self.builtin_rate == nil then
                -- 只在首次接管时捕获（setRefreshRate(0) 会持久化 0，重复捕获会把
                -- 0 当成"原值"）；捕获到 0 视为 0.1.7 接管残留，按默认 6 还原。
                self.builtin_rate = UIManager.full_refresh_count
                    or (G_reader_settings and G_reader_settings:readSetting("full_refresh_count"))
                    or 6
                if self.builtin_rate == 0 then
                    logger.warn("K4Refresh: 捕获到内置全刷档 0（疑为 v0.1.7 接管残留），还原时按 6")
                    self.builtin_rate = 6
                end
            end
            UIManager:setRefreshRate(0)
            -- setRefreshRate 会把 0 持久化进设置；立即把持久化值写回接管前档，
            -- 运行态（UIManager 缓存字段）保持 0，重启后不会把 0 当用户设置。
            if G_reader_settings then
                pcall(G_reader_settings.saveSetting, G_reader_settings,
                    "full_refresh_count", self.builtin_rate)
            end
            logger.info("K4Refresh: KOReader builtin full-refresh disabled (plugin owns policy)")
        else
            local restored = self.builtin_rate or 6
            UIManager:setRefreshRate(restored)
            self.builtin_rate = nil
            logger.info(string.format(
                "K4Refresh: ghost off -> KOReader builtin full-refresh restored to %d", restored))
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
                text = "Quantize now (16-level, no flash)",
                keep_menu_open = false,
                callback = function()
                    -- 手动一次性：量化当前屏并按 partial 呈现（CLI 路径，与钩子无关）
                    os.execute("/mnt/us/k4refresh/k4refresh-cli fxquant --refresh partial >/dev/null 2>&1")
                end,
            },
            {
                text = "Duokan quant: On",
                checked_func = function() return self.quant end,
                callback = function() self:setQuant(true) end,
            },
            {
                text = "Duokan quant: Off",
                checked_func = function() return not self.quant end,
                callback = function() self:setQuant(false) end,
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
