default:
    just --list

# Build debug firmware
build:
    cargo build

# Build release firmware.
# On the first build after adding a custom partition table, cmake creates the
# out/ dir during its configure step but then fails to find partitions.csv
# during the build step. The retry copies it there and succeeds.
build-release:
    #!/usr/bin/env bash
    shopt -s nullglob
    _sync() {
        # Cargo's build-dir layout is build/esp-idf-sys-<hash>/ on older
        # toolchains and build/esp-idf-sys/<hash>/ on newer ones.
        for dir in target/riscv32imac-esp-espidf/release/build/esp-idf-sys-*/out \
                   target/riscv32imac-esp-espidf/release/build/esp-idf-sys/*/out; do
            cp -f partitions.csv "$dir/"
        done
    }
    _sync
    cargo build --release || { _sync && cargo build --release; }

# Flash release firmware and open serial monitor
flash port="": (_flash "release" port)

# Flash debug firmware and open serial monitor
flash-debug port="": (_flash "debug" port)

# The IDF-built bootloader lives in esp-idf-sys's build dir; older cargo
# layouts also copied it to target/<triple>/<profile>/bootloader.bin.
_flash profile port:
    #!/usr/bin/env bash
    set -euo pipefail
    shopt -s nullglob
    dir=target/riscv32imac-esp-espidf/{{profile}}
    boot=($dir/bootloader.bin $dir/build/esp-idf-sys*/out/build/bootloader/bootloader.bin $dir/build/esp-idf-sys/*/out/build/bootloader/bootloader.bin)
    [ ${#boot[@]} -gt 0 ] || { echo "bootloader.bin not found — build first"; exit 1; }
    espflash flash \
        {{ if port != "" { "--port " + port } else { "" } }} \
        --bootloader "${boot[0]}" \
        --partition-table partitions.csv \
        --erase-data-parts ota \
        $dir/amoled-kurokku-esp \
        --monitor

# Build release and flash in one step
deploy: build-release flash

# Open the serial monitor without flashing
monitor:
    espflash monitor

# Run unit tests on the host (pure modules only). Overrides the default ESP
# target from .cargo/config.toml.
test:
    cargo test --target {{ `rustc -vV | awk '/^host/ {print $2}'` }}

# Render a clock face to PNG on the host.
# Usage: just preview [HH:MM:SS] [12h] | message "text" [up] [small|medium|large] [@ms] | status "TITLE" "detail"
[positional-arguments]
preview *args:
    cargo run --example preview --target {{ `rustc -vV | awk '/^host/ {print $2}'` }} -- "$@"

# Provision a device's NVS from tools/devices/<name>.yaml.
# Usage: just provision amoled0 [/dev/cu.usbmodem2101]
provision device port="":
    uv run tools/provision.py {{device}} {{ if port != "" { "--port " + port } else { "" } }}
