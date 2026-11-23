//! "Let the assistant listen to audio" (parity roadmap G20): a short, text-only description
//! of what some audio sounds like, measured on the machine, that an assistant turn can include
//! so answers can refer to the sound.
//!
//! [`analyze`] takes interleaved stereo samples and measures integrated loudness (ITU-R
//! BS.1770 K-weighting and gating), sample peak, RMS and crest factor, how the energy splits
//! over four bands and the spectral centroid, a key guess (chroma against major and minor key
//! profiles) and a tempo guess (autocorrelation of an onset envelope). Only the
//! [`Summary::text`] line ever leaves the machine; the samples never do.

use std::f64::consts::PI;

/// Longest stretch [`analyze`] looks at; callers render at most this much.
pub const MAX_SECONDS: f64 = 30.0;

const NOTE_NAMES: [&str; 12] = ["C", "C#", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B"];
// Krumhansl-Kessler key profiles, tonic first.
const MAJOR: [f64; 12] = [6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88];
const MINOR: [f64; 12] = [6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17];

/// Band edges in Hz: low, low mids, high mids, highs.
pub const BANDS: [(f64, f64, &str); 4] =
    [(20.0, 250.0, "low"), (250.0, 2000.0, "low-mid"), (2000.0, 6000.0, "high-mid"), (6000.0, 20000.0, "high")];

#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    pub seconds: f64,
    /// Below about -70 dBFS RMS: nothing else is measured.
    pub silent: bool,
    /// Integrated loudness, LUFS (None when every block is gated away).
    pub lufs: Option<f64>,
    pub peak_dbfs: f64,
    pub rms_dbfs: f64,
    /// Share of the energy in each of [`BANDS`], 0..1.
    pub bands: [f64; 4],
    /// Spectral centroid, Hz.
    pub centroid_hz: f64,
    /// (pitch class 0 = C, minor, correlation -1..1).
    pub key: Option<(usize, bool, f64)>,
    pub bpm: Option<f64>,
}

impl Summary {
    pub fn key_name(&self) -> Option<String> {
        self.key.map(|(pc, minor, _)| format!("{} {}", NOTE_NAMES[pc], if minor { "minor" } else { "major" }))
    }

    /// One paragraph for the assistant: what was measured, rounded to what matters.
    pub fn text(&self) -> String {
        let mut s = format!("{:.1} s analysed. ", self.seconds);
        if self.silent {
            s.push_str("It is silent.");
            return s;
        }
        match self.lufs {
            Some(l) => s.push_str(&format!("Integrated loudness {l:.1} LUFS")),
            None => s.push_str("Loudness too low to measure"),
        }
        s.push_str(&format!(
            ", sample peak {:.1} dBFS, RMS {:.1} dBFS, crest factor {:.1} dB. ",
            self.peak_dbfs,
            self.rms_dbfs,
            self.peak_dbfs - self.rms_dbfs
        ));
        let bands: Vec<String> =
            BANDS.iter().zip(self.bands).map(|((lo, hi, name), v)| format!("{name} ({lo:.0}-{hi:.0} Hz) {:.0}%", v * 100.0)).collect();
        s.push_str(&format!("Energy by band: {}. Spectral centroid {:.0} Hz. ", bands.join(", "), self.centroid_hz));
        match (self.key_name(), self.key) {
            (Some(k), Some((_, _, c))) => s.push_str(&format!("Key guess {k} (confidence {c:.2}). ")),
            _ => s.push_str("No clear key. "),
        }
        match self.bpm {
            Some(b) => s.push_str(&format!("Tempo guess {b:.0} BPM.")),
            None => s.push_str("No clear beat."),
        }
        s
    }
}

/// Measures interleaved stereo `samples` at `rate` Hz (at most [`MAX_SECONDS`] of them are used).
pub fn analyze(samples: &[f32], rate: f64) -> Summary {
    let max = (MAX_SECONDS * rate) as usize * 2;
    let samples = &samples[..samples.len().min(max) & !1];
    let frames = samples.len() / 2;
    let seconds = frames as f64 / rate.max(1.0);
    let mut peak = 0.0f64;
    let mut sum = 0.0f64;
    for &x in samples {
        peak = peak.max(f64::from(x).abs());
        sum += f64::from(x) * f64::from(x);
    }
    let rms = if samples.is_empty() { 0.0 } else { (sum / samples.len() as f64).sqrt() };
    let db = |v: f64| if v > 0.0 { 20.0 * v.log10() } else { -150.0 };
    let mut out = Summary {
        seconds,
        silent: db(rms) < -70.0,
        lufs: None,
        peak_dbfs: db(peak),
        rms_dbfs: db(rms),
        bands: [0.0; 4],
        centroid_hz: 0.0,
        key: None,
        bpm: None,
    };
    if out.silent {
        return out;
    }
    out.lufs = loudness(samples, rate);
    let mono: Vec<f64> = samples.chunks_exact(2).map(|f| 0.5 * (f64::from(f[0]) + f64::from(f[1]))).collect();
    spectrum(&mono, rate, &mut out);
    out.bpm = tempo(&mono, rate);
    out
}

// ---------------------------------------------------------------- loudness (BS.1770)

#[derive(Clone, Copy)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    z: [f64; 2],
}

impl Biquad {
    fn run(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

/// The two K-weighting stages for any rate (the BS.1770 filters, designed from their analogue
/// prototypes as in libebur128).
fn k_weighting(rate: f64) -> [Biquad; 2] {
    let (f0, g, q) = (1681.974450955533, 3.999843853973347, 0.7071752369554196);
    let k = (PI * f0 / rate).tan();
    let vh = 10f64.powf(g / 20.0);
    let vb = vh.powf(0.4996667741545416);
    let a0 = 1.0 + k / q + k * k;
    let shelf = Biquad {
        b: [(vh + vb * k / q + k * k) / a0, 2.0 * (k * k - vh) / a0, (vh - vb * k / q + k * k) / a0],
        a: [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
        z: [0.0; 2],
    };
    let (f0, q) = (38.13547087602444, 0.5003270373238773);
    let k = (PI * f0 / rate).tan();
    let a0 = 1.0 + k / q + k * k;
    let high_pass = Biquad { b: [1.0, -2.0, 1.0], a: [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0], z: [0.0; 2] };
    [shelf, high_pass]
}

/// Gated integrated loudness: 400 ms blocks every 100 ms, -70 LUFS absolute and -10 LU relative gates.
fn loudness(samples: &[f32], rate: f64) -> Option<f64> {
    let frames = samples.len() / 2;
    let step = (0.1 * rate).round() as usize;
    if step == 0 || frames < 4 * step {
        return None;
    }
    // Mean square of the K-weighted signal per 100 ms step, both channels summed.
    let mut filters = [k_weighting(rate), k_weighting(rate)];
    let mut steps = Vec::with_capacity(frames / step);
    let mut acc = 0.0;
    for (i, f) in samples.chunks_exact(2).enumerate() {
        for (c, x) in f.iter().enumerate() {
            let [a, b] = &mut filters[c];
            let y = b.run(a.run(f64::from(*x)));
            acc += y * y;
        }
        if (i + 1) % step == 0 {
            steps.push(acc / step as f64);
            acc = 0.0;
        }
    }
    let blocks: Vec<f64> = steps.windows(4).map(|w| w.iter().sum::<f64>() / 4.0).collect();
    let lk = |z: f64| -0.691 + 10.0 * z.log10();
    let gated = |thresh: f64| {
        let kept: Vec<f64> = blocks.iter().copied().filter(|&z| z > 0.0 && lk(z) > thresh).collect();
        (!kept.is_empty()).then(|| kept.iter().sum::<f64>() / kept.len() as f64)
    };
    let abs = gated(-70.0)?;
    gated(lk(abs) - 10.0).map(lk)
}

// ---------------------------------------------------------------- spectrum, key

const N: usize = 4096;

/// In-place radix-2 FFT (`re.len()` a power of two).
fn fft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * PI / len as f64;
        let (wr, wi) = (ang.cos(), ang.sin());
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0, 0.0);
            for k in 0..len / 2 {
                let (a, b) = (start + k, start + k + len / 2);
                let (tr, ti) = (re[b] * cr - im[b] * ci, re[b] * ci + im[b] * cr);
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                (cr, ci) = (cr * wr - ci * wi, cr * wi + ci * wr);
            }
        }
        len <<= 1;
    }
}

fn power_spectra(mono: &[f64], n: usize, hop: usize, mut each: impl FnMut(&[f64])) {
    let window: Vec<f64> = (0..n).map(|i| 0.5 - 0.5 * (2.0 * PI * i as f64 / n as f64).cos()).collect();
    let (mut re, mut im, mut pow) = (vec![0.0; n], vec![0.0; n], vec![0.0; n / 2]);
    let mut start = 0;
    while start + n <= mono.len().max(n) {
        for i in 0..n {
            re[i] = mono.get(start + i).copied().unwrap_or(0.0) * window[i];
            im[i] = 0.0;
        }
        fft(&mut re, &mut im);
        for k in 0..n / 2 {
            pow[k] = re[k] * re[k] + im[k] * im[k];
        }
        each(&pow);
        start += hop;
    }
}

fn spectrum(mono: &[f64], rate: f64, out: &mut Summary) {
    let bin_hz = rate / N as f64;
    let mut total = vec![0.0; N / 2];
    power_spectra(mono, N, N / 2, |p| {
        for (t, v) in total.iter_mut().zip(p) {
            *t += v;
        }
    });
    let (mut sum, mut weighted) = (0.0, 0.0);
    let mut chroma = [0.0f64; 12];
    for (k, &p) in total.iter().enumerate().skip(1) {
        let hz = k as f64 * bin_hz;
        if let Some(b) = BANDS.iter().position(|&(lo, hi, _)| hz >= lo && hz < hi) {
            out.bands[b] += p;
        }
        if (20.0..20000.0).contains(&hz) {
            sum += p;
            weighted += p * hz;
        }
        if (55.0..5000.0).contains(&hz) {
            let midi = 69.0 + 12.0 * (hz / 440.0).log2();
            chroma[(midi.round() as i64).rem_euclid(12) as usize] += p.sqrt();
        }
    }
    let band_sum: f64 = out.bands.iter().sum();
    if band_sum > 0.0 {
        out.bands.iter_mut().for_each(|b| *b /= band_sum);
    }
    out.centroid_hz = if sum > 0.0 { weighted / sum } else { 0.0 };
    out.key = key(&chroma);
}

fn correlation(a: &[f64; 12], b: &[f64; 12]) -> f64 {
    let (ma, mb) = (a.iter().sum::<f64>() / 12.0, b.iter().sum::<f64>() / 12.0);
    let (mut num, mut da, mut db) = (0.0, 0.0, 0.0);
    for i in 0..12 {
        num += (a[i] - ma) * (b[i] - mb);
        da += (a[i] - ma).powi(2);
        db += (b[i] - mb).powi(2);
    }
    if da <= 0.0 || db <= 0.0 { 0.0 } else { num / (da * db).sqrt() }
}

/// Best of the 24 keys, if its profile correlates at least 0.5 with the chroma.
fn key(chroma: &[f64; 12]) -> Option<(usize, bool, f64)> {
    let mut best = (0, false, f64::MIN);
    for tonic in 0..12 {
        for (minor, profile) in [(false, &MAJOR), (true, &MINOR)] {
            let rotated: [f64; 12] = std::array::from_fn(|i| profile[(i + 12 - tonic) % 12]);
            let c = correlation(chroma, &rotated);
            if c > best.2 {
                best = (tonic, minor, c);
            }
        }
    }
    (best.2 >= 0.5).then_some(best)
}

// ---------------------------------------------------------------- tempo

/// Tempo from the autocorrelation of a spectral-flux onset envelope (1024-sample frames,
/// 512 hop), searched over 60-200 BPM with a gentle preference for 120. None without a
/// clear periodic beat.
fn tempo(mono: &[f64], rate: f64) -> Option<f64> {
    let (n, hop) = (1024, 512);
    if mono.len() < rate as usize * 3 {
        return None;
    }
    let mut prev = vec![0.0; n / 2];
    let mut env = Vec::with_capacity(mono.len() / hop);
    power_spectra(mono, n, hop, |p| {
        let mut flux = 0.0;
        for (q, &v) in prev.iter_mut().zip(p) {
            let m = v.sqrt();
            flux += (m - *q).max(0.0);
            *q = m;
        }
        env.push(flux);
    });
    let mean = env.iter().sum::<f64>() / env.len() as f64;
    env.iter_mut().for_each(|v| *v = (*v - mean).max(0.0));
    let energy: f64 = env.iter().map(|v| v * v).sum();
    if energy <= 0.0 {
        return None;
    }
    let fps = rate / hop as f64;
    let lag_of = |bpm: f64| fps * 60.0 / bpm;
    let (lo, hi) = (lag_of(200.0).floor() as usize, lag_of(60.0).ceil() as usize);
    let ac = |lag: usize| env.iter().zip(&env[lag.min(env.len())..]).map(|(a, b)| a * b).sum::<f64>() / energy;
    let mut best = (0usize, 0.0f64, 0.0f64);
    for lag in lo.max(1)..=hi.min(env.len() / 2) {
        let raw = ac(lag);
        let bpm = 60.0 * fps / lag as f64;
        let weight = (-0.5 * ((bpm / 120.0).log2() / 0.9).powi(2)).exp();
        if raw * weight > best.2 {
            best = (lag, raw, raw * weight);
        }
    }
    if best.0 == 0 || best.1 < 0.1 {
        return None;
    }
    // Refine between neighbouring lags with a parabola through the three autocorrelations.
    let (l, c) = (best.0, best.1);
    let (a, b) = (ac(l - 1), ac(l + 1));
    let denom = a - 2.0 * c + b;
    let shift = if denom.abs() > 1e-12 { (0.5 * (a - b) / denom).clamp(-0.5, 0.5) } else { 0.0 };
    Some(60.0 * fps / (l as f64 + shift))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f64 = 48000.0;

    fn stereo(secs: f64, f: impl Fn(f64) -> f64) -> Vec<f32> {
        (0..(secs * RATE) as usize)
            .flat_map(|i| {
                let v = f(i as f64 / RATE) as f32;
                [v, v]
            })
            .collect()
    }

    #[test]
    fn a_sine_measures_its_level_and_brightness() {
        // 1 kHz at -20 dBFS on both channels reads -20 LUFS (BS.1770's reference point).
        let s = analyze(&stereo(5.0, |t| 0.1 * (2.0 * PI * 1000.0 * t).sin()), RATE);
        let lufs = s.lufs.expect("measured");
        assert!((lufs + 20.0).abs() < 0.3, "{lufs}");
        assert!((s.peak_dbfs + 20.0).abs() < 0.05);
        assert!((s.rms_dbfs + 23.01).abs() < 0.05);
        assert!((s.centroid_hz - 1000.0).abs() < 50.0, "{}", s.centroid_hz);
        assert!(s.bands[1] > 0.95, "{:?}", s.bands);
    }

    #[test]
    fn a_minor_chord_reads_as_a_minor() {
        let hz = [220.0, 261.63, 329.63, 440.0];
        let s = analyze(&stereo(4.0, |t| hz.iter().map(|f| 0.1 * (2.0 * PI * f * t).sin()).sum()), RATE);
        assert_eq!(s.key_name().as_deref(), Some("A minor"), "{:?}", s.key);
    }

    #[test]
    fn clicks_at_120_bpm_read_as_120() {
        let beat = 0.5;
        let s = analyze(
            &stereo(10.0, |t| {
                let p = t % beat;
                if p < 0.03 { (2.0 * PI * 2000.0 * t).sin() * (1.0 - p / 0.03) * 0.5 } else { 0.0 }
            }),
            RATE,
        );
        let bpm = s.bpm.expect("a beat");
        assert!((bpm - 120.0).abs() < 2.0, "{bpm}");
    }

    #[test]
    fn silence_says_so_and_nothing_else() {
        let s = analyze(&vec![0.0; 96000], RATE);
        assert!(s.silent && s.lufs.is_none() && s.key.is_none());
        assert!(s.text().ends_with("It is silent."));
        // A sustained tone has no beat; the text says so instead of guessing.
        let tone = analyze(&stereo(4.0, |t| 0.2 * (2.0 * PI * 440.0 * t).sin()), RATE);
        assert!(tone.bpm.is_none(), "{:?}", tone.bpm);
        assert!(tone.text().contains("No clear beat."));
    }
}
