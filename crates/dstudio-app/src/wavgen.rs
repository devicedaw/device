//! WAV writer (16/24-bit PCM: export, test signals) and the `--demo` test signals. Generated at 44.1 kHz on
//! purpose so the demo also exercises the engine's resampler (engine runs at 48 kHz).

use std::io::Write;
use std::path::Path;

pub fn write_wav(path: &Path, rate: u32, channels: u16, samples: &[f32]) -> std::io::Result<()> {
    write_wav_pcm(path, rate, channels, 16, samples)
}

/// Interleaved `samples` as 16- or 24-bit integer PCM.
pub fn write_wav_pcm(path: &Path, rate: u32, channels: u16, bits: u16, samples: &[f32]) -> std::io::Result<()> {
    encode_wav_pcm(std::fs::File::create(path)?, rate, channels, bits, samples)
}

/// `write_wav_pcm` into an already opened file.
pub fn encode_wav_pcm(file: std::fs::File, rate: u32, channels: u16, bits: u16, samples: &[f32]) -> std::io::Result<()> {
    let bytes = (bits / 8) as u32;
    let data_len = u32::try_from(samples.len() as u64 * bytes as u64)
        .ok()
        .filter(|&n| n <= u32::MAX - 36)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "too long for a WAV file (4 GB limit)"))?;
    let mut f = std::io::BufWriter::new(file);
    f.write_all(b"RIFF")?;
    f.write_all(&(36 + data_len).to_le_bytes())?;
    f.write_all(b"WAVEfmt ")?;
    f.write_all(&16u32.to_le_bytes())?;
    f.write_all(&1u16.to_le_bytes())?; // PCM
    f.write_all(&channels.to_le_bytes())?;
    f.write_all(&rate.to_le_bytes())?;
    f.write_all(&(rate * channels as u32 * bytes).to_le_bytes())?;
    f.write_all(&(channels * bytes as u16).to_le_bytes())?;
    f.write_all(&bits.to_le_bytes())?;
    f.write_all(b"data")?;
    f.write_all(&data_len.to_le_bytes())?;
    let full = ((1i32 << (bits - 1)) - 1) as f32;
    for s in samples {
        let v = (s.clamp(-1.0, 1.0) * full).round() as i32;
        f.write_all(&v.to_le_bytes()[..bytes as usize])?;
    }
    f.flush()
}

/// 4 s mono: a 220 Hz sine with a beat-synchronous (120 BPM) decaying envelope.
pub fn sine_bursts(rate: u32) -> Vec<f32> {
    let n = rate as usize * 4;
    (0..n)
        .map(|i| {
            let t = i as f32 / rate as f32;
            let ph = (t * 2.0).fract(); // beats at 120 BPM
            let env = 0.15 + 0.75 * (-ph * 5.0).exp();
            env * (std::f32::consts::TAU * 220.0 * t).sin()
        })
        .collect()
}

/// 3 s stereo (interleaved): white noise with a slow tremolo, different per channel.
pub fn noise(rate: u32) -> Vec<f32> {
    let n = rate as usize * 3;
    let mut s: u32 = 0x1234_5678;
    let mut out = Vec::with_capacity(n * 2);
    for i in 0..n {
        let t = i as f32 / rate as f32;
        for ch in 0..2 {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            let r = s as f32 / u32::MAX as f32 * 2.0 - 1.0;
            let trem = 0.35 + 0.3 * (t * std::f32::consts::TAU * (0.75 + ch as f32 * 0.25)).sin();
            out.push(r * trem);
        }
    }
    out
}

/// Minimal WAV reader (PCM 16/24/32-bit, IEEE float 32-bit, incl. WAVE_FORMAT_EXTENSIBLE):
/// returns (sample rate, channels, interleaved samples). Used by the self-test to
/// decode recorded takes independently of the engine.
pub fn read_wav(path: &Path) -> Result<(u32, u16, Vec<f32>), String> {
    let b = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if b.len() < 12 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return Err("not a RIFF/WAVE file".into());
    }
    let u16_at = |i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
    let u32_at = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    let (mut fmt, mut data) = (None, None);
    let mut i = 12;
    while i + 8 <= b.len() {
        let id = &b[i..i + 4];
        let len = u32_at(i + 4) as usize;
        let body = i + 8;
        let end = (body + len).min(b.len());
        // A chunk cut short by the end of the file is read only as far as the file goes.
        if id == b"fmt " && len >= 16 && end >= body + 16 {
            let mut tag = u16_at(body);
            if tag == 0xFFFE && len >= 26 && end >= body + 26 {
                tag = u16_at(body + 24); // sub-format GUID starts with the format tag
            }
            fmt = Some((tag, u16_at(body + 2), u32_at(body + 4), u16_at(body + 14)));
        } else if id == b"data" {
            data = Some(&b[body..end]);
        }
        i = body + len + (len & 1);
    }
    let (tag, ch, rate, bits) = fmt.ok_or("no fmt chunk")?;
    let d = data.ok_or("no data chunk")?;
    let s: Vec<f32> = match (tag, bits) {
        (3, 32) => d.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect(),
        (1, 16) => d.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0).collect(),
        (1, 24) => d.chunks_exact(3).map(|c| ((i32::from_le_bytes([0, c[0], c[1], c[2]])) >> 8) as f32 / 8_388_608.0).collect(),
        (1, 32) => d.chunks_exact(4).map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]]) as f32 / 2_147_483_648.0).collect(),
        _ => return Err(format!("unsupported WAV format tag {tag}, {bits} bits")),
    };
    Ok((rate, ch.max(1), s))
}

#[cfg(test)]
mod tests {
    #[test]
    fn write_read_roundtrip() {
        let p = std::env::temp_dir().join(format!("dstudio-wav-rt-{}.wav", std::process::id()));
        let s: Vec<f32> = (0..100).map(|i| (i as f32 / 50.0) - 1.0).collect();
        super::write_wav(&p, 48_000, 2, &s).unwrap();
        let (r, ch, back) = super::read_wav(&p).unwrap();
        assert_eq!((r, ch, back.len()), (48_000, 2, 100));
        assert!(back.iter().zip(&s).all(|(a, b)| (a - b).abs() < 1e-3));
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn pcm_rounds_to_the_nearest_step() {
        let p = std::env::temp_dir().join(format!("dstudio-wav-round-{}.wav", std::process::id()));
        // 0.9 and -0.9 of a 16-bit step round to ±1; truncation would write 0 for both.
        let step = 1.0 / 32767.0;
        super::write_wav(&p, 48_000, 1, &[0.9 * step, -0.9 * step, 0.4 * step, 1.5]).unwrap();
        let b = std::fs::read(&p).unwrap();
        let got: Vec<i16> = b[44..].chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
        assert_eq!(got, vec![1, -1, 0, 32767]);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn a_truncated_fmt_chunk_is_an_error_not_a_panic() {
        let p = std::env::temp_dir().join(format!("dstudio-wav-cut-{}.wav", std::process::id()));
        let mut b = b"RIFF\0\0\0\0WAVEfmt ".to_vec();
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes()); // 2 of the 16 bytes the chunk claims
        std::fs::write(&p, &b).unwrap();
        assert!(super::read_wav(&p).is_err());
        b.truncate(20);
        b.extend_from_slice(&[0xFE, 0xFF, 2, 0, 0x80, 0xBB, 0, 0, 0, 0, 0, 0, 4, 0, 16, 0]); // extensible, cut before its GUID
        let mut fixed = b.clone();
        fixed[16..20].copy_from_slice(&40u32.to_le_bytes());
        std::fs::write(&p, &fixed).unwrap();
        assert!(super::read_wav(&p).is_err());
        let _ = std::fs::remove_file(p);
    }
}
