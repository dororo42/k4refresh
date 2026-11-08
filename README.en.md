# k4refresh — Kindle 4 E-Ink Refresh Policy Controller

English | [简体中文](readme.md)

A Rust-written refresh optimization layer for the Kindle 4 (legacy `einkfb`): **page turns keep KOReader's native partial refresh, while the KOReader plugin runs a deterministic `fx_update_slow` full refresh every N pages (ghost budget) to clear ghosting** — plus on-demand manual full refresh, region clamping, and an ioctl-timed benchmark.

- 🆕 **v0.1.10 (2026-11-07)**: **Duokan-parity quantization** — reverse engineering of the Duokan firmware (see the RE report) proved its renderer quantizes every pixel with `v&0xF0|v>>4` (16 gray levels, idempotent), which caps partial-refresh grayscale contamination at 16 levels. New: CLI `fxquant` subcommand (quantize visible framebuffer + optional refresh), KOReader plugin "Duokan quant" toggle (takes over einkfb `refreshPartialImp/refreshFullImp`, quantizing in place with pure LuaJIT FFI after blit / before ioctl — no `.so` needed), 3 KUAL menu items (Quantize now / Duokan Quant On/Off), `mode.conf` line 2 `quant on|off`, FFI export `k4refresh_quant_refresh`.
- 🆕 **v0.1.9 (2026-11-07)**: KUAL menu nested into a sub-menu (top level shows "K4Refresh ▸"); fixed the legacy `config.xml` menu not appearing under KUAL 2.7 (parse.awk only honors `<menu type="json">`). CLI binary unchanged from v0.1.8.
- ✅ **Status: v0.1.10 released after on-device acceptance (2026-11-08)** — a crossed two-device A/B (K4/K5 panels swapped) showed the quantized ladder fits with 10–15% lower residual RMS and ~20% faster full-page partial refresh; **recommended daily config: `ghost 4` + `quant on`** (accepted by the user). Known characterization: quantization is a *drift upper-bound controller* (locks contamination at 16 levels), not a ghost eliminator — large white↔black transitions still leave mid-gray residue, which the periodic slow refresh cleans up.
- 📦 Download: [Releases](https://github.com/dororo42/k4refresh/releases) zip (static CLI / Lua bridge / KOReader plugin / KUAL extension / SHA256SUMS; since v0.1.8 no `.so` or dynamic CLI is shipped); or Actions → build → artifacts.
- 🤖 CI: every push builds ARM artifacts; `v*` tags publish a Release automatically.

---

## 1. zip contents (Release)

```text
k4refresh-cli            ← static CLI (musl, preferred, zero dependencies)
k4refresh.koplugin/      ← KOReader plugin: ghost-priority mode (slow refresh every N pages)
lua/k4refresh.lua        ← KOReader bridge script (manual/console use)
kual/config.xml          ← KUAL menu registration (type="json" pointing to menu.json)
kual/menu.json           ← KUAL menu items (KUAL 2.7 json format)
kual/bin/k4r.sh          ← KUAL menu script
kual/bin/k4refresh-cli   ← CLI copy used by KUAL
INSTALL.txt              ← deployment quick reference (same as §3)
SHA256SUMS.txt           ← integrity check
```

> Since v0.1.8 (Plan A) `libk4refresh.so` and the dynamic CLI are no longer shipped: the
> K4 userland is armel (glibc 2.12.1); gnueabihf binaries cannot be loaded (see §7).
> FFI returns once the .so is rebuilt with koxtoolchain armel (then
> `touch /mnt/us/k4refresh/ffi.enabled` re-enables probing).

Verify with: `sha256sum -c SHA256SUMS.txt`.

## 2. Build (if self-building)

```bash
./build.sh              # musl static CLI (the only shipped artifact)
cargo test              # policy unit tests (host)
```

## 3. Deployment (copy-paste; prerequisites: jailbreak + KUAL; SSH channel, pick one)

> **KUAL recommendation**: [dororo42/KUAL_Booklet](https://github.com/dororo42/KUAL_Booklet) (KUAL-neo — faithful rebuild of official v2.7 plus UI optimizations, Release `v2.7.37-neo.1`). The K4Refresh extension has been tested on both KUAL-neo and stock KUAL 2.7; both acceptance devices (K4/K5) currently run KUAL-neo.

SSH channel (pick one; A recommended):
- **A. KOReader's built-in SSH server** (no usbnet needed, smallest attack surface): KOReader → gear → Network → Wireless → start SSH server; default port `2222`, user `root`, password visible/changeable in KOReader settings
- B. USBNetwork: install the usbnet package, then `;debugOn` → `~usbNetwork` → `;debugOff`

```bash
# From the unzipped directory (adjust IP/port; add -P 2222 to scp)
ssh -P 2222 root@192.168.2.x "mkdir -p /mnt/us/k4refresh /mnt/us/extensions/k4refresh/bin /mnt/us/koreader/plugins"
scp -P 2222 k4refresh-cli                 root@192.168.2.x:/mnt/us/k4refresh/
scp -P 2222 k4refresh-cli                 root@192.168.2.x:/mnt/us/extensions/k4refresh/bin/
scp -P 2222 kual/config.xml               root@192.168.2.x:/mnt/us/extensions/k4refresh/
scp -P 2222 kual/menu.json                root@192.168.2.x:/mnt/us/extensions/k4refresh/
scp -P 2222 kual/bin/k4r.sh               root@192.168.2.x:/mnt/us/extensions/k4refresh/bin/
scp -P 2222 lua/k4refresh.lua             root@192.168.2.x:/mnt/us/koreader/   # lowercase filename
scp -P 2222 -r k4refresh.koplugin         root@192.168.2.x:/mnt/us/koreader/plugins/
ssh -P 2222 root@192.168.2.x "chmod +x /mnt/us/k4refresh/* /mnt/us/extensions/k4refresh/bin/* && sync"
```

> After deploying/updating the KUAL extension, **reboot the Kindle** (KUAL scans extensions/ only at boot).

## 4. Usage

### 4.1 KOReader plugin (ghost-priority mode, main entry)

After restarting KOReader, the main menu shows **K4Refresh (ghost clearing)**:

| Menu item | Action |
|---|---|
| Full Refresh (slow) | Immediate full-screen slow refresh (ghost clear) |
| Quantize now (16-level, no flash) | One-shot: quantize the visible fb + partial refresh (runs the CLI, independent of the toggle) |
| Duokan quant: On / Off | v0.1.10: automatic quantization toggle. When On, the plugin takes over einkfb `refreshPartialImp/refreshFullImp` and quantizes the visible fb in place (pure LuaJIT FFI on KOReader's already-mmap'd fb, no .so); Off restores the original functions |
| Ghost clearing: Off / Every 4 / 6 / 8 pages | Slow full refresh every N pages (effective immediately; written to mode.conf shared with KUAL/CLI) |

Behavior: page turns stay native partial; every N page turns trigger one slow full refresh (one flash, ghosting zeroed); large jumps (chapters, |Δpage|>1) flush immediately. Since v0.1.8 (Plan A) FFI is disabled by default (no loadable .so on K4 armel) and the plugin falls back to the static CLI — cleanup never breaks. The quantization hook is unrelated to that FFI switch (it uses KOReader's own LuaJIT + already-mapped memory, no .so involved).

**Recommended daily config (v0.1.10, device-accepted)**: `ghost 4` + `quant on` (works on both K4/K5; the K5 plugin must be upgraded to v0.1.10 to gain the quantization hook).

**Built-in refresh takeover (v0.1.7+, corrected in v0.1.8)**: while ghost mode is active, the plugin sets KOReader's built-in "full refresh every 6 partials" (UIManager `full_refresh_count`) to 0, preventing double flashes; Ghost: Off restores the value captured before takeover (v0.1.7 hardcoded 6 and could clobber user settings; since v0.1.8 the original value is captured and 0 is never persisted). Background: measured ghosting accumulates monotonically across consecutive partial full-screen inversions (RMS ~10%→15% from the 1st to 4th inversion) — a periodic slow refresh is necessary by design.

### 4.2 KUAL menu

KUAL → K4Refresh (writes mode.conf, effective **after the next book open**; the in-KOReader menu is immediate):

| Menu item | Action |
|---|---|
| AB-A no-quant (stay) | Ghosting A/B test: lays the unquantized gray-band pattern and **leaves it on screen** (`bench --keep`; reopen KOReader or `flash` to restore) |
| AB-B quant (stay) | Same, quantized pattern |
| Full Refresh (slow) | Immediate full-screen slow refresh |
| Quantize now (16-level) | CLI `fxquant --refresh partial`: quantize the current screen and show it |
| Duokan Quant: On / Off | Writes `quant on/off` (mode.conf line 2), effective next book open |
| Ghost: Off | Writes `off`: disables automatic cleanup |
| Ghost: Every 4 / 6 / 8 Pages | Writes `ghost N`: cleanup every N pages |
| Screen Info | On-screen resolution/depth (600x800, bpp=8 is normal) |
| Benchmark (CSV) | Runs bench, results in `/mnt/us/k4refresh/bench.csv` |

### 4.3 Command line (SSH)

```bash
/mnt/us/k4refresh/k4refresh-cli --version                  # print version
/mnt/us/k4refresh/k4refresh-cli info                       # smoke step 1: 600x800, bpp=8
/mnt/us/k4refresh/k4refresh-cli flash                      # forced slow full refresh
/mnt/us/k4refresh/k4refresh-cli fxquant --refresh partial  # v0.1.10+: 16-level quantize (idempotent) + partial refresh
/mnt/us/k4refresh/k4refresh-cli fxquant --refresh none     # quantize only (shown on next refresh)
/mnt/us/k4refresh/k4refresh-cli restore --mode 1           # v0.1.7+: RESTORE_DISPLAY full-frame replay (glitch repair / deterministic cleanup)
/mnt/us/k4refresh/k4refresh-cli fxupdate --mode 1 --which -1            # v0.1.7+: full-screen refresh (fx_t channel)
/mnt/us/k4refresh/k4refresh-cli fxupdate --mode 0 --which 21 --exclude 0,750,600,800
#                                                          ↑ partial + invert transform + exclude bottom 50 rows
/mnt/us/k4refresh/k4refresh-cli refresh --mode fast --interval 6
/mnt/us/k4refresh/k4refresh-cli bench --fx partial,fast,slow --n 20 --label T --out /mnt/us/bench.csv
/mnt/us/k4refresh/k4refresh-cli bench --seq fast,fast,fast,slow --n 20 --label C --out /mnt/us/bench_seq.csv
#                                                          ↑ combined sequence timing: simulates a real "3 fast + 1 cleanup" page-turn cycle
/mnt/us/k4refresh/k4refresh-cli bench --fx slow --n 3 --label A --delay-ms 3000 --out /dev/null
#                                                       ↑ 3 s between frames for visual marker identification
/mnt/us/k4refresh/k4refresh-cli bench --fx partial --n 3 --delay-ms 7000 --pattern gray --shot-idx 1 --label N
#                                                       ↑ grayscale band ghosting observation; --quant adds 16-level quantization; --shot-idx is 0-based (frame N+1 shows the "please photograph" banner)
/mnt/us/k4refresh/k4refresh-cli bench --fx partial --n 3 --delay-ms 2000 --pattern gray --quant --shot-idx 2 --label B --keep
#                                                       ↑ v0.1.10+: --keep does not restore the previous screen; the banner frame (B3) stays for photography; reopen KOReader / flash to restore
#
# bench notes (v0.1.2+):
#   - each refresh carries a counter marker (A1/A2/A3…) in the top-left corner for visual counting
#   - --delay-ms N (v0.1.3+): pause between refreshes; without it frames fire back-to-back
#   - --sync (v0.1.5+): fsync(fd) (= WAITFORVSYNC) timing per refresh; the ioctl itself is synchronous
#     (~125 ms), fsync only 0.4–0.5 ms; CSV gains a sync_usec column
#   - --pattern gray / --quant / --shot-idx (v0.1.5/0.1.6+): ghosting comparison tools, see examples above
#   - --keep (v0.1.10+): do not restore the previous screen at the end (photo mode; restore via flash or reopening KOReader)
#   - the previous screen is restored automatically at the end (unless --keep)
#   - final message is "N rows written to xxx (M failed ioctls)"; only M>0 needs attention
#
# restore/fxupdate notes (v0.1.7+): ioctl numbers auto-probe new (0x46ef/0x46e4) → old (0x4644/0x4642)
# and report which one took effect; on K4 4.1.4 the new numbers work directly.
```

### 4.4 Inside KOReader (Lua bridge, manual mode)

KOReader menu → tools → Lua console:

```lua
local K4R = require("k4refresh")   -- lowercase filename
K4R.init()                          -- idempotent; reads mode.conf written by KUAL
K4R.flash()                         -- immediate full refresh
K4R.quant("partial")                -- v0.1.10+: quantize + refresh ("none"/"full"/"slow" optional)
K4R.set_mode(0, 6)                  -- fast mode, cleanup every 6 pages
K4R.set_mode(1)                     -- back to conservative
```

These calls can be bound to gestures/keys.

## 5. Uninstall (100% stock restore)

```bash
ssh root@192.168.2.x
rm -rf /mnt/us/extensions/k4refresh              # KUAL menu
rm -rf /mnt/us/koreader/plugins/k4refresh.koplugin  # KOReader plugin
rm -f  /mnt/us/koreader/k4refresh.lua            # Lua bridge
rm -rf /mnt/us/k4refresh                         # libs, CLI, mode.conf, experiment data
sync
```

No system-partition writes, no resident components — deleting is enough. Since v0.1.8 the plugin's takeover does not pollute the `full_refresh_count` setting: after uninstall, restarting KOReader restores the built-in refresh rate; to force it immediately, run `require("ui/uimanager"):setRefreshRate(6)` in the Lua console.

## 6. Troubleshooting

| Symptom | Fix |
|---|---|
| `info` cannot open /dev/fb0 | Not jailbroken or USBNet off; a root shell is required |
| `info` resolution is not 600x800 | Stop and uninstall immediately; report `/proc/version` |
| `require("k4refresh")` fails in KOReader | Ensure the file is in `/mnt/us/koreader/` with the lowercase name |
| KUAL menu item does nothing | Ensure `extensions/k4refresh/bin/` contains `k4r.sh` and `k4refresh-cli`, both executable |
| Visible ghosting | Lower the ghost interval (Every 4) or use Full Refresh manually; image-heavy books may want larger intervals |

More: tech-design doc §7 full test matrix, §8 backup/rollback, §9 risk list.

## 7. Known boundaries

- Kindle 4 only (Non-Touch, FW 4.1.4, legacy einkfb); not applicable to mxcfb devices.
- The v0.1.7 plugin takes over the *cleanup schedule* only: page turns remain KOReader's native partial; the plugin triggers a slow cleanup every N pages and disables KOReader's built-in every-6-pages promotion while ghost mode is active (double-flash prevention; restored when Off). Rewriting KOReader's refresh backend to use the fast tier is not implemented — on-device tests showed the three fx types are visually indistinguishable on real page turns, and each channel's ioctl is synchronous at ~125 ms, so that path has no payoff and was closed.
- **FFI disabled by default (Plan A, implemented 2026-11-01; characterized on-device 2026-09-19)**: the K4 userland is armel/softfp + glibc 2.12.1; gnueabihf artifacts (armhf + GLIBC_2.18~2.34 tags) cannot be dlopen'd (reproduced on-device: `ld-linux-armhf.so.3` missing). Since v0.1.8 the zip ships no .so or dynamic CLI; the plugin/Lua bridge use the static CLI directly (flash smoke-tested on device). The FFI path returns with a koxtoolchain armel rebuild (Plan B); then `touch /mnt/us/k4refresh/ffi.enabled` re-enables probing without code changes.
- KUAL changes take effect after the next book open; the in-KOReader menu is immediate.
- The fast/conservative library modes only affect refreshes issued through this library (CLI refresh / Lua bridge `K4R.refresh`); no page-turn consumer exists (v0.1.x legacy interface kept for ABI compatibility).
- Windows cannot build this crate (`libc::ioctl` is POSIX-only); use Linux or CI for tests/builds.
