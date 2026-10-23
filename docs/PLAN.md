# AMOLED Kurokku ESP: Plan

Rust firmware for the Waveshare ESP32-C6-Touch-AMOLED-2.16 (480×480 AMOLED, 3-button case).
It joins the kurokku family as a sibling of `led-kurokku-esp` and is driven by `kurokku-esp-server`.

## 1. Hardware facts (from vendor code in `../ESP32-C6-Touch-AMOLED-2.16`)

| Thing | Detail |
|---|---|
| MCU | ESP32-C6, single-core RISC-V @160 MHz, **512 KB SRAM, no PSRAM**, 16 MB flash |
| Display | CO5300 (SH8601-compatible command set), 480×480 RGB565, **QSPI** on SPI2: PCLK=GPIO0, D0–D3=GPIO1–4, **CS=GPIO15**. No reset pin: reset is done by toggling AXP2101 **ALDO3**. Brightness is set with command `0x51` (0–255) |
| Touch | CST9217/CST9220 on I2C (0x5A), INT=GPIO5, RST=GPIO11 (needs swap_xy + **mirror_x**; the vendor's mirror_y is 180° off with our MADCTL). **INT never reads low in practice**, so poll over I2C (~20 ms) |
| I2C bus | SCL=GPIO7, SDA=GPIO8: AXP2101 PMIC (0x34), PCF85063 RTC, QMI8658 IMU, ES8311/ES7210 audio, touch |
| microSD | **Shares SPI2 with the display** (CLK=0, MOSI=1, MISO=2), CS=GPIO6, SDSPI mode |
| Audio | ES8311 + ES7210, I2S on GPIO19–23 (not needed at first) |
| Buttons | **BOOT** = GPIO9, active-low (strapping pin, normal input after boot) · **KEY** = GPIO10, active-low · **PWR** = AXP2101 PWRON (short-press IRQ over I2C; long press ~4 s = hardware power-off), also readable on GPIO18 (idles low, high while pressed) |
| Battery | **None fitted.** USB-powered only; battery/charging UI is out of scope |
| Temperature | **No ambient sensor.** The QMI8658 and the chip die temperatures read high. Temperature has to come from the server (nalssi → Redis) |

Notes:
- The vendor `user_config.h` says LCD CS=5 and TP INT=15, but the code that actually runs (`display_bsp.h` defaults, XiaoZhi board) uses **CS=15, INT=5**. Trust the code.
- Two LCD init sequences exist. Use the one in the ESP-IDF example `09_LVGL_V9_Test/components/port_bsp/display_bsp.cpp`. The XiaoZhi 2.16 board has a column window of 0x16..0x1AF, which looks copied from the 2.06" board.
- Partial-window writes must start on an even X/Y and end on an odd X/Y. The vendor "rounder" callback enforces this.

## 2. The design constraint: no framebuffer

480×480×2 bytes = **460 KB**. That's almost all of the SRAM, and WiFi, lwIP and TLS need about 150–200 KB of it. So:

- **Render in horizontal strips.** Use two DMA buffers of about 480×32 px (30 KB each) in ping-pong. Draw a strip into one buffer while the other is sent over QSPI.
- **Redraw only dirty rectangles.** A clock changes a few digit boxes per second, not the whole screen.
- **Keep large art in flash.** The 16 MB of flash is plentiful. Nixie and flip-clock digits are pre-rendered RGB565 sprites in a data partition, blitted directly. Store them raw or RLE so they can be read in strips.
- **Stream images.** Never hold a whole image in RAM: decode a few rows → push → repeat.

Throughput: 40 MHz × 4 lines works out to about 23 ms per full frame in theory, so roughly 20–40 fps full-screen. Animating small regions is much faster.

## 3. Tech choices (recommended)

- **`esp-idf-svc` (std)**, same as `led-kurokku-esp`. That project's config/NVS, wifi, polling, OTA, udp_log and provisioning code mostly ports unchanged. ESP-IDF also provides FAT-on-SD, an HTTP client, SNTP and `esp_lcd`.
  - Target `riscv32imac-esp-espidf`, `MCU=esp32c6`, nightly plus `build-std`.
  - Alternative: no_std `esp-hal` with embassy gives more RAM and control, but we'd rebuild WiFi/HTTP/OTA/FAT ourselves. Not worth it here.
- **Display driver**: a small Rust CO5300 driver on top of ESP-IDF's `esp_lcd_panel_io_spi` in quad mode (raw `esp-idf-sys` bindings), using the vendor init table. No C component is needed.
- **Graphics**: our own strip renderer (`src/gfx.rs`). A `Scene` paints any region on demand; the `Renderer` walks dirty rects in 480×32 bands. Text uses **anti-aliased fonts** (AMOLED makes jaggies obvious): `build.rs` rasterizes TTFs from `assets/fonts/` into 4-bit alpha glyph atlases with `fontdue`. `embedded-graphics` can be added behind `Canvas` when faces need shapes (flip cards, nixie tubes).
- **Host preview**: rendering is pure (no ESP deps), so `just preview [HH:MM:SS] [12h]` renders a face to PNG on the Mac in seconds instead of a flash cycle. A unit test checks that incremental (dirty-rect) redraws match a full render pixel for pixel. This follows the "pure modules are host-testable" pattern from `led-kurokku-esp`.
- **Image format on the wire/SD**: **QOI** (simple, streamable, small decoder) or raw RGB565. The server converts and resizes everything to 480×480. JPEG directly from the SD card can come later via `esp_jpeg`/tjpgd, which decodes in strips.

## 4. Phases

### Phase 0: Bring-up (prove the hardware from Rust)
- Cargo project skeleton: target, partitions (2× OTA app, NVS, ~8 MB `assets` partition), sdkconfig, Justfile.
- AXP2101 init (DC1/ALDO rails, ALDO3 display reset), CO5300 init, fill screen with colors, brightness sweep.
- Read RTC; read touch coordinates to the log.
- **Button probe**: log level changes on candidate GPIOs plus the AXP2101 PWRON IRQ, to find KEY. *(Done: KEY=GPIO10, PWR also on GPIO18.)*
- ✅ Done when: colors on screen, brightness works, all 3 buttons identified and logged. **Done 2026-10-22**: orientation and colors correct, touch mapping fixed (180° off with vendor flags), full-screen fill 30 ms, 435 KB free heap before WiFi.

### Phase 1: Rendering core + first clock (offline)
- Strip renderer and dirty rects; build-time font atlases (Bebas Neue at 3 sizes).
- **"Modern" clock**: big anti-aliased digits plus date, time from the PCF85063 RTC (works with no WiFi).
- Host PNG preview (`just preview`).
- Buttons: BOOT cycles brightness, KEY toggles 12/24h, PWR turns the panel off/on.
- ✅ Done when: the device boots straight to a good-looking clock without a network. Measured: full frame 87 ms, minute rollover 18 ms, 402 KB free heap. Until NTP (Phase 2), an unset RTC is seeded from the build time.

### Phase 2: Connectivity (port from led-kurokku-esp)
- NVS config plus `provision.py`, WiFi, SNTP → **write back to RTC**, udp syslog, HTTP polling, OTA.
- `display_type=amoled480`. Brightness 0–15 maps to 0–255 along a perceptual curve.
- **Message widget**: static/centered when it fits, otherwise a horizontal marquee or (`direction: "up"`) a word-wrapped vertical crawl; `color` (`#RRGGBB`) and `size` (`small`/`medium`/`large`). `scroll_speed_ms` keeps its LED meaning (ms per matrix column), scaled so text crosses the screen as fast as it crosses a 32-column matrix.
- **Temperature** = message with `redis_key` (nalssi already writes it). A dedicated `info` widget can come later.
- **Status screens** at boot (KUROKKU → WIFI → ONLINE + IP); unsupported instruction types (animation, raw_*) fall back to the clock.
- Buttons: KEY now also dismisses a message back to the clock.
- Server (`kurokku-esp-server`, branch `amoled480`): new display type in the device form; `style` (clock) and `direction`/`color`/`size` (message) in the model, resolver and playlist form, so UI edits keep them.
- ✅ Done when: server playlists drive clock ⇄ messages ⇄ temperature on the device. **Done 2026-10-22**: device `amoled0` on the live server with playlist `amoled-test`; boot to engine 6 s, NTP within 3 s, 241 KB free heap with WiFi, app image 2.4 MB of the 3 MB slot.

- **Smooth scrolling** (2026-10-22): the panel's TE line isn't wired (GPIO14 probed), so there's no vsync. Instead each frame is written well inside one ~16.7 ms refresh: wire-order canvas, a glyph fast path (only antialiased edges blend), and double-buffered bands so drawing overlaps DMA. Marquee went from 22 fps / 36 ms per frame to ~75 fps / 12 ms (3 ms DMA wait).

### Phase 3: Clock styles
`style` field on the clock instruction (needs kurokku-esp-server PR #10 deployed); on the device, KEY cycles faces and the choice persists in NVS (`clock_style`), with a server `style` taking precedence.
- `modern`: Phase 1.
- `led`: 7-segment digits rasterized at build time (`build_support/segments.rs`) as glyphs: lit segments with blurred halos, unlit "ghost" segments, blinking colon, 7-segment seconds.
- `flip`: split-flap cards; on change the old top half folds down and the new bottom half swings in (600 ms, ~48 fps). Uses new Canvas primitives: clipping, AA rounded gradient rects, vertically squashed glyphs.
- `nixie`: tube art generated at build time (`build_support/nixie.rs`): one RGB565 tube background (glass, mesh, stacked unlit cathodes) plus a 4-bit glow map per digit, colored through an orange→white-hot palette and added at runtime; neon colon dots; 200 ms crossfade on change.
- **Burn-in protection**: every face shifts around an 8-point, ±3 px orbit every 2 minutes, stepping at :30 so the repaint never lands on a minute rollover. Full repaints keep flip/fade animations going.
- ✅ **Done 2026-10-22.** App image is now 2.68 MB of the 3 MB slot, so Phase 4 starts by moving fonts/art into the assets partition.
- Later ideas: color options per face, nixie cathode-cycling, date/temperature complications.

### Phase 4: Pictures
- `image` instruction: `{type:"image", url, fit}` → stream QOI over HTTP to the screen.
- microSD: mount FAT on the shared SPI bus, run a slideshow from `/sdcard/images/`, and make it a server-selectable source (`{type:"slideshow", source:"sd", interval_secs}`).
- Server: image library — add images on the server (admin UI), which converts them (resize/crop to 480², encode QOI), caches them and serves them by URL. The device only ever pulls; it never accepts uploads.
- Later: animated images (QOI frame sequences), JPEG from SD.

### Phase 5: Interaction
Suggested default mapping (configurable later):

| Button | Short press | Long press |
|---|---|---|
| **KEY** | next widget / next clock style | status screen (IP, firmware, RSSI) |
| **BOOT** | cycle brightness (or toggle night mode) | snooze/ack current alert |
| **PWR** | screen off/on (AMOLED black = ~0 power) | *(hardware power-off, 4 s)* |

- Touch: swipe left/right = previous/next widget, tap = show info overlay.
- IMU: auto-rotate to case orientation; optionally wake on pickup or tap.
- Report events to the server (`POST …/events`) so the server can react, e.g. "ack alert" or "skip playlist entry".

### Phase 6: Server and scheduling
- Extend **kurokku-esp-server** rather than writing a new one. It already has playlists, cron entries, alerts, day/night brightness from lat/long (`internal/solar`), OTA and the admin UI.
- Add: amoled widget fields (style, colors, direction, image url), image library, device event endpoint, quiet hours (screen off), and an admin UI preview of what the device shows.

## 5. Decisions
- No battery is fitted: no battery or charging features.
- Pictures are pulled from the server by URL (or read from microSD). The device runs no upload endpoint.
- Stack: `esp-idf-svc` (std), extending `kurokku-esp-server`.

## 6. Open questions
- KEY button GPIO: found by the Phase 0 probe (GPIO10).
