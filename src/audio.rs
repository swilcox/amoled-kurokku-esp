//! Sound: the ES8311 codec (I2C 0x18) driving the speaker from I2S0.
//!
//! I2S0: MCLK=19 (256 × sample rate), BCLK=20, WS=22, DOUT=23; the codec is
//! an I2S slave clocked from MCLK. No amplifier enable pin on this board.
//!
//! `spawn` starts an audio thread; `Audio::play` hands it a WAV URL (as
//! for images: http(s)://, sd:/…) to stream to the speaker. A new sound
//! cuts off the one playing. Between sounds the codec is powered down.
//!
//! Register sequences follow Espressif's esp_codec_dev ES8311 driver, as
//! used by the vendor demo (DAC only, slave mode, MCLK in, 16-bit I2S).

use std::io::Read;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};

use anyhow::{anyhow, bail, Result};
use esp_idf_svc::hal::delay::BLOCK;
use esp_idf_svc::hal::gpio::{InputPin, OutputPin};
use esp_idf_svc::hal::i2s::config::{DataBitWidth, StdConfig};
use esp_idf_svc::hal::i2s::{I2s, I2sDriver, I2sTx};

use crate::input::SharedI2c;
use crate::{media, wav};

const ADDR: u8 = 0x18;
/// Sample rates the codec has clock settings for at MCLK = 256 × rate.
const RATES: [u32; 8] = [8000, 11025, 16000, 22050, 24000, 32000, 44100, 48000];
/// Stereo frames per write to I2S.
const CHUNK_FRAMES: usize = 512;

struct Es8311 {
    i2c: SharedI2c,
}

impl Es8311 {
    fn write(&self, reg: u8, val: u8) -> Result<()> {
        self.i2c.lock().unwrap().write(ADDR, &[reg, val], BLOCK).map_err(|e| anyhow!("ES8311 write {reg:#04x}: {e}"))
    }

    fn read(&self, reg: u8) -> Result<u8> {
        let mut b = [0u8];
        self.i2c.lock().unwrap().write_read(ADDR, &[reg], &mut b, BLOCK).map_err(|e| anyhow!("ES8311 read {reg:#04x}: {e}"))?;
        Ok(b[0])
    }

    fn update(&self, reg: u8, f: impl FnOnce(u8) -> u8) -> Result<()> {
        let v = self.read(reg)?;
        self.write(reg, f(v))
    }

    /// Power-on configuration (esp_codec_dev `es8311_open`).
    fn open(&self) -> Result<()> {
        // Noise immunity on the I2C lines; written twice as the first
        // write after power-up can fail.
        let _ = self.write(0x44, 0x08);
        self.write(0x44, 0x08)?;
        for (reg, val) in [
            (0x01, 0x30),
            (0x02, 0x00),
            (0x03, 0x10),
            (0x16, 0x24),
            (0x04, 0x10),
            (0x05, 0x00),
            (0x0B, 0x00),
            (0x0C, 0x00),
            (0x10, 0x1F),
            (0x11, 0x7F),
            (0x00, 0x80), // slave mode
            (0x01, 0x3F), // clocks on, from MCLK, not inverted
        ] {
            self.write(reg, val)?;
        }
        self.update(0x06, |v| v & !0x20)?; // SCLK not inverted
        self.write(0x13, 0x10)?;
        self.write(0x1B, 0x0A)?;
        self.write(0x1C, 0x6A)?;
        self.write(0x44, 0x58)?;
        self.suspend()
    }

    /// Clocking, format and power-up for 16-bit I2S at `rate`, then the
    /// DAC unmuted at `volume` (`set_fs`, `es8311_start`, `set_vol`).
    fn start(&self, rate: u32, volume: u8) -> Result<()> {
        // 16-bit, standard I2S framing.
        self.update(0x09, |v| (v | 0x0C) & 0xFC)?;
        self.update(0x0A, |v| (v | 0x0C) & 0xFC)?;
        // MCLK = 256 × rate: pre-divider and multiplier 1, ADC/DAC
        // dividers 1, single speed, LRCK = MCLK/256, BCLK = MCLK/4.
        self.update(0x02, |v| v & 0x07)?;
        self.write(0x05, 0x00)?;
        self.update(0x03, |v| (v & 0x80) | 0x10)?;
        let dac_osr = if rate <= 16000 { 0x20 } else { 0x10 };
        self.update(0x04, |v| (v & 0x80) | dac_osr)?;
        self.update(0x07, |v| v & 0xC0)?;
        self.write(0x08, 0xFF)?;
        self.update(0x06, |v| (v & 0xE0) | 0x03)?;
        // Start: DAC in, ADC off.
        self.write(0x00, 0x80)?;
        self.write(0x01, 0x3F)?;
        self.update(0x09, |v| v & !0x40)?;
        self.update(0x0A, |v| v | 0x40)?;
        for (reg, val) in [(0x17, 0xBF), (0x0E, 0x02), (0x12, 0x00), (0x14, 0x1A), (0x0D, 0x01), (0x15, 0x40), (0x37, 0x08), (0x45, 0x00)] {
            self.write(reg, val)?;
        }
        self.write(0x32, volume_reg(volume))?;
        self.update(0x31, |v| v & 0x9F) // unmute
    }

    /// Power down between sounds (`es8311_suspend`).
    fn suspend(&self) -> Result<()> {
        for (reg, val) in [
            (0x32, 0x00),
            (0x17, 0x00),
            (0x0E, 0xFF),
            (0x12, 0x02),
            (0x14, 0x00),
            (0x0D, 0xFA),
            (0x15, 0x00),
            (0x02, 0x10),
            (0x00, 0x00),
            (0x00, 0x1F),
            (0x01, 0x30),
            (0x01, 0x00),
            (0x45, 0x00),
            (0x0D, 0xFC),
            (0x02, 0x00),
        ] {
            self.write(reg, val)?;
        }
        Ok(())
    }
}

/// DAC volume register for `volume` 0-100: 0 is silent, 1-100 spans
/// -50 dB to 0 dB (the register is 0.5 dB steps from -95.5 dB).
fn volume_reg(volume: u8) -> u8 {
    match volume.min(100) as u32 {
        0 => 0,
        v => ((-50.0 + 50.0 * v as f32 / 100.0 + 95.5) * 2.0) as u8,
    }
}

/// Handle to the audio thread.
#[derive(Clone)]
pub struct Audio {
    tx: Sender<String>,
}

impl Audio {
    /// Play the WAV at `url` (cutting off any sound already playing).
    pub fn play(&self, url: &str) {
        let _ = self.tx.send(url.to_string());
    }
}

/// Configure the codec and I2S0 and start the audio thread. `volume` is
/// 0-100.
pub fn spawn<'d: 'static>(
    i2c: SharedI2c,
    i2s: impl I2s + 'd,
    mclk: impl InputPin + OutputPin + 'd,
    bclk: impl InputPin + OutputPin + 'd,
    ws: impl InputPin + OutputPin + 'd,
    dout: impl OutputPin + 'd,
    volume: u8,
) -> Result<Audio> {
    let codec = Es8311 { i2c };
    codec.open()?;
    let mut i2s = I2sDriver::<I2sTx>::new_std_tx(i2s, &StdConfig::philips(24000, DataBitWidth::Bits16), bclk, dout, Some(mclk), ws)?;
    i2s.tx_enable()?;
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("audio".into())
        .stack_size(8 * 1024)
        .spawn(move || run(codec, i2s, rx, volume))?;
    log::info!("audio: ES8311 ready, volume {volume}");
    Ok(Audio { tx })
}

fn run(codec: Es8311, mut i2s: I2sDriver<'static, I2sTx>, rx: Receiver<String>, volume: u8) {
    let mut rate = 24000;
    let mut next = None;
    loop {
        let url = match next.take() {
            Some(u) => u,
            None => match rx.recv() {
                Ok(u) => u,
                Err(_) => return,
            },
        };
        match play(&codec, &mut i2s, &mut rate, &url, volume, &rx) {
            Ok(cut_off_by) => next = cut_off_by,
            Err(e) => log::warn!("sound {url}: {e}"),
        }
        if let Err(e) = codec.suspend() {
            log::warn!("audio: {e}");
        }
    }
}

/// Stream one sound. Returns the URL of a sound that interrupted it.
fn play(codec: &Es8311, i2s: &mut I2sDriver<'static, I2sTx>, rate: &mut u32, url: &str, volume: u8, rx: &Receiver<String>) -> Result<Option<String>> {
    let started = std::time::Instant::now();
    let mut r = media::open(url)?;
    let fmt = wav::read_header(&mut r)?;
    if !RATES.contains(&fmt.sample_rate) {
        bail!("unsupported sample rate {} Hz", fmt.sample_rate);
    }
    if fmt.sample_rate != *rate {
        i2s.tx_reconfigure_std(&StdConfig::philips(fmt.sample_rate, DataBitWidth::Bits16))?;
        *rate = fmt.sample_rate;
    }
    codec.start(fmt.sample_rate, volume)?;
    log::info!("sound {url}: {} Hz, {} ch, {} ms", fmt.sample_rate, fmt.channels, fmt.data_len as u64 * 1000 / (fmt.sample_rate as u64 * 2 * fmt.channels as u64));

    let mut left = fmt.data_len as usize;
    let mut pcm = vec![0u8; CHUNK_FRAMES * 2 * fmt.channels as usize];
    let mut out = Vec::with_capacity(CHUNK_FRAMES * 4);
    while left > 0 {
        match rx.try_recv() {
            Ok(u) => return Ok(Some(u)),
            Err(TryRecvError::Disconnected) => return Ok(None),
            Err(TryRecvError::Empty) => {}
        }
        let want = left.min(pcm.len());
        let n = read_full(&mut r, &mut pcm[..want])?;
        if n == 0 {
            break;
        }
        left -= n;
        wav::to_stereo(&pcm[..n & !1], fmt.channels, 256, &mut out);
        i2s.write_all(&out, BLOCK)?;
    }
    // Let the DMA buffers drain with silence before powering down.
    out.clear();
    out.resize(CHUNK_FRAMES * 4, 0);
    for _ in 0..4 {
        i2s.write_all(&out, BLOCK)?;
    }
    log::info!("sound {url}: done in {} ms", started.elapsed().as_millis());
    Ok(None)
}

/// Read until `buf` is full or the stream ends.
fn read_full(r: &mut impl Read, buf: &mut [u8]) -> Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..])? {
            0 => break,
            k => n += k,
        }
    }
    Ok(n)
}
