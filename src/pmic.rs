//! Minimal AXP2101 power-management driver.
//!
//! Only what this board needs: power rails up, display reset via ALDO3, and
//! the PWR button's short/long-press IRQ flags. Register values mirror the
//! vendor `power_bsp.cpp` / XiaoZhi board init. No battery is fitted, so the
//! charger is left at its defaults.

use anyhow::{anyhow, Result};
use esp_idf_svc::hal::delay::{FreeRtos, BLOCK};
use esp_idf_svc::hal::i2c::I2cDriver;

use crate::board::ADDR_PMIC;

const REG_CHIP_ID: u8 = 0x03;
const REG_VBUS_CUR_LIM: u8 = 0x16;
const REG_DC_ONOFF: u8 = 0x80;
const REG_DC1_VOLT: u8 = 0x82;
const REG_LDO_ONOFF0: u8 = 0x90;
const REG_ALDO1_VOLT: u8 = 0x92;
const REG_INTEN2: u8 = 0x41;
const REG_INTSTS1: u8 = 0x48;
const REG_INTSTS2: u8 = 0x49;
const REG_INTSTS3: u8 = 0x4A;

const CHIP_ID_AXP2101: u8 = 0x4A;
const VBUS_CUR_LIM_2000MA: u8 = 0x05;
const ALDO3_BIT: u8 = 1 << 2;
// IRQ bits 10 (long press) and 11 (short press) live in INTEN2/INTSTS2.
const PKEY_LONG_BIT: u8 = 1 << 2;
const PKEY_SHORT_BIT: u8 = 1 << 3;

/// PWR button events latched by the PMIC since the last read.
#[derive(Debug, Default, Clone, Copy)]
pub struct PowerKey {
    pub short: bool,
    pub long: bool,
}

fn read(i2c: &mut I2cDriver, reg: u8) -> Result<u8> {
    let mut buf = [0u8];
    i2c.write_read(ADDR_PMIC, &[reg], &mut buf, BLOCK)
        .map_err(|e| anyhow!("axp2101 read 0x{reg:02x}: {e}"))?;
    Ok(buf[0])
}

fn write(i2c: &mut I2cDriver, reg: u8, val: u8) -> Result<()> {
    i2c.write(ADDR_PMIC, &[reg, val], BLOCK)
        .map_err(|e| anyhow!("axp2101 write 0x{reg:02x}: {e}"))
}

fn update(i2c: &mut I2cDriver, reg: u8, f: impl FnOnce(u8) -> u8) -> Result<()> {
    let v = read(i2c, reg)?;
    write(i2c, reg, f(v))
}

/// Bring up the rails: DC1 and ALDO1-4 at 3.3V, all enabled.
pub fn init(i2c: &mut I2cDriver) -> Result<()> {
    let id = read(i2c, REG_CHIP_ID)?;
    if id != CHIP_ID_AXP2101 {
        log::warn!("axp2101: unexpected chip id 0x{id:02x}");
    }

    update(i2c, REG_VBUS_CUR_LIM, |v| (v & !0x07) | VBUS_CUR_LIM_2000MA)?;
    write(i2c, REG_DC1_VOLT, ((3300 - 1500) / 100) as u8)?;
    update(i2c, REG_DC_ONOFF, |v| v | 0x01)?;
    for i in 0..4 {
        write(i2c, REG_ALDO1_VOLT + i, ((3300 - 500) / 100) as u8)?;
    }
    update(i2c, REG_LDO_ONOFF0, |v| v | 0x0F)?;

    update(i2c, REG_INTEN2, |v| v | PKEY_LONG_BIT | PKEY_SHORT_BIT)?;
    for reg in [REG_INTSTS1, REG_INTSTS2, REG_INTSTS3] {
        write(i2c, reg, 0xFF)?;
    }

    log::info!(
        "axp2101: id=0x{id:02x} dc=0x{:02x} ldo=0x{:02x}",
        read(i2c, REG_DC_ONOFF)?,
        read(i2c, REG_LDO_ONOFF0)?
    );
    Ok(())
}

fn set_aldo3(i2c: &mut I2cDriver, on: bool) -> Result<()> {
    update(i2c, REG_LDO_ONOFF0, |v| if on { v | ALDO3_BIT } else { v & !ALDO3_BIT })
}

/// The panel has no reset GPIO; it is reset by power-cycling ALDO3.
pub fn reset_display(i2c: &mut I2cDriver) -> Result<()> {
    for on in [true, false, true] {
        set_aldo3(i2c, on)?;
        FreeRtos::delay_ms(100);
    }
    Ok(())
}

/// Read and clear the PWR button IRQ flags.
pub fn take_power_key(i2c: &mut I2cDriver) -> Result<PowerKey> {
    let sts = read(i2c, REG_INTSTS2)?;
    let mask = sts & (PKEY_LONG_BIT | PKEY_SHORT_BIT);
    if mask != 0 {
        write(i2c, REG_INTSTS2, mask)?;
    }
    Ok(PowerKey {
        short: mask & PKEY_SHORT_BIT != 0,
        long: mask & PKEY_LONG_BIT != 0,
    })
}
