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
- `railway` / `railway-black` (added 2026-10-23): station-clock dial with bold bars; the red lollipop seconds hand sweeps round in 58.5 s, rests at 12, and sets off as the minute hand jumps. `classic`: charcoal sunburst dial, Libre Baskerville numerals, tapered cream hands ticking each second, date window at 3. Dials are pre-rendered row-RLE in the asset pack (`build_support/analog.rs`); hands are drawn per pixel from a fixed-point distance function, row spans only, with non-overlapping dirty bands; the sweep runs at ~50–60 fps.
- **Burn-in protection**: every face shifts around an 8-point, ±3 px orbit every 2 minutes, stepping at :30 so the repaint never lands on a minute rollover. Full repaints keep flip/fade animations going.
- ✅ **Done 2026-10-22.** App image is now 2.68 MB of the 3 MB slot, so Phase 4 starts by moving fonts/art into the assets partition.
- Later ideas: color options per face, nixie cathode-cycling, date/temperature complications.

### Phase 4: Pictures
- **Assets partition** (prerequisite): fonts and face art moved out of the app (2.68 → 1.54 MB) into an asset pack built by `build_support/assetpack.rs` and memory-mapped at boot. Two slots, `assets_0`/`assets_1` (A/B like OTA): the firmware maps whichever holds the pack hash it was built with. OTA of `x.bin` also fetches `x.assets.bin` (if published) into the inactive slot, hash-verified, header written last. App rollback is enabled: an OTA'd app without its pack reboots into the previous app. Both paths tested on hardware.
- **Images**: QOI, decoded row by row straight onto the panel (`src/qoi.rs`, `src/image.rs`) — no frame buffer; the panel holds the picture. Centered on black if small, center-cropped if large. `{type:"image", url}` with `http(s)://…` or `sd:/path.qoi`. ~0.9 s per 480² photo over WiFi, 0.6–0.8 s from SD.
- **microSD**: mounted at `/sdcard` as a second device on the display's SPI bus. `{type:"slideshow", path:"sd:/images", interval_secs:10}` cycles the `.qoi` files in a folder.
- `tools/img2qoi.py`: any JPEG/PNG/HEIC → 480² QOI (cover-crop, or `--fit` to letterbox).
- Server (next): `image`/`slideshow` playlist widgets, plus an image library that converts uploads to 480² QOI and serves them by URL. The device only ever pulls.
- Later: animated images (QOI frame sequences), JPEG from SD.

### Phase 5: Interaction
Mapping as built (the table in `src/engine.rs` is the reference):

| Input | Action |
|---|---|
| **KEY** | next clock face / close overlay / dismiss to the clock |
| **KEY** hold | info screen (IP, RSSI, server health, firmware, uptime, heap, rotation) |
| **BOOT** | cycle brightness |
| **BOOT** hold | lock rotation where it is / back to auto (persisted as `rotation`) |
| **PWR** | screen off; any button wakes it. *(Hold 4 s: hardware power-off.)* |
| tap | info screen on/off |
| swipe left / right | next / previous playlist entry (server unreachable: clock face) |
| swipe up / down | brighter / dimmer |

- Buttons report short presses on release and long presses (800 ms) while still held (`buttons::PressTracker`).
- Touch and the QMI8658 accelerometer are polled by an `input` thread (50 Hz / 5 Hz) that shares the I2C bus behind a mutex, so 3 ms touch reads never stall the display thread. `touch::Gestures` turns points into tap / long press / swipe; a dropped sample mid-touch doesn't end it.
- **Rotation** is done in software: the CO5300 only mirrors X, so the renderer turns each band on its way out (`gfx::Rotation`: 180° reverses the band; 90/270 transpose through a 30 KB scratch buffer). Touches map back through the same rotation. Auto-rotate needs 0.6 g in the screen plane held for 1 s; flat or diagonal keeps the current rotation. Config `rotation`: `auto` (default) or fixed `0|90|180|270`.
- Overlays (info, notices) cover the active widget; server changes wait until they close, then the widget (or image) is repainted.
- ✅ **Done 2026-10-23** (firmware). All controls and all four orientations checked on the device.
- **Device events**: swipes `POST /api/v1/devices/{id}/events {"event":"next"|"prev"}`; the server moves the device along its playlist (fresh duration, cron-inactive entries passed over) and the network thread polls right away, so the change shows in well under a second. Checked on the device against the server branch `device-events`.
- Later: "ack alert" (touch long press is still free).

### Phase 6: Server and scheduling
- Extend **kurokku-esp-server** rather than writing a new one. It already has playlists, cron entries, alerts, day/night brightness from lat/long (`internal/solar`), OTA and the admin UI.
- Add: amoled widget fields (style, colors, direction, image url), image library, device event endpoint, quiet hours (screen off), and an admin UI preview of what the device shows.
- Done 2026-10-23: widget fields and the image library (server PRs #10, #11), device events (#12), then quiet hours and the admin preview:
  - A device's `quiet_cron` (server time, evaluated per minute like entry crons) puts `display_off: true` in every poll response while it matches; an invalid cron never matches.
  - The firmware turns the panel off; any button wakes it until a minute after the last input (`quiet.rs`). When quiet hours end the panel comes back on.
  - The admin status panel previews amoled480 content: clock face style, message color and direction, library image thumbnail, slideshow folder, and "screen off" during quiet hours.

## 5. Decisions
- No battery is fitted: no battery or charging features.
- Pictures are pulled from the server by URL (or read from microSD). The device runs no upload endpoint.
- Stack: `esp-idf-svc` (std), extending `kurokku-esp-server`.

## 6. Open questions
- KEY button GPIO: found by the Phase 0 probe (GPIO10).
