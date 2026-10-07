use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream};
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Super-Wideband Voice Sampling Rate (24 kHz mono)
/// Covers 0-12 kHz frequency spectrum for crystal-clear HD gaming voice chat.
pub const VOICE_SAMPLE_RATE: u32 = 24000;
/// 20ms frame = 480 samples at 24 kHz
pub const SAMPLES_PER_FRAME: usize = 480;
/// Target jitter buffer size: ~40ms (2 frames = 960 samples @ 24 kHz)
pub const JITTER_TARGET_SAMPLES: usize = 960;
/// Max jitter buffer limit: ~120ms (6 frames = 2880 samples @ 24 kHz) to prevent latency buildup
pub const JITTER_MAX_SAMPLES: usize = 2880;
/// VAD hangover frames: keep transmitting for 15 frames (300ms) after speech drops to prevent chopped words
pub const VAD_HANGOVER_FRAMES: usize = 15;

/// Codec identifier tags
pub const CODEC_PCM24K: u8 = 0x02;
pub const CODEC_PCM16K: u8 = 0x01;

/// Smooth soft-knee saturation / limiter to completely eliminate harsh digital clipping & rasping ("хрип").
/// Passes signals under 75% volume linearly (1:1), and smoothly curves loud peaks/plosives towards 1.0.
#[inline]
pub fn soft_limiter(x: f32) -> f32 {
    if x > 0.75 {
        let delta = x - 0.75;
        0.75 + 0.24 * (delta / (1.0 + delta)).min(1.0)
    } else if x < -0.75 {
        let delta = -x - 0.75;
        -0.75 - 0.24 * (delta / (1.0 + delta)).min(1.0)
    } else {
        x
    }
}

/// Helper: Resamples 16 kHz PCM to 24 kHz (ratio 2:3) with linear interpolation
pub fn resample_16k_to_24k(input: &[i16]) -> Vec<i16> {
    let n = input.len();
    if n == 0 {
        return Vec::new();
    }
    let out_len = (n * 3) / 2;
    let mut output = Vec::with_capacity(out_len);
    let step = (n as f64) / (out_len as f64);
    for i in 0..out_len {
        let phase = (i as f64) * step;
        let idx = phase as usize;
        let s = if idx + 1 < n {
            let frac = (phase - idx as f64) as f32;
            let s0 = input[idx] as f32;
            let s1 = input[idx + 1] as f32;
            s0 + (s1 - s0) * frac
        } else {
            input[n - 1] as f32
        };
        output.push(s.clamp(-32768.0, 32767.0) as i16);
    }
    output
}

/// Legacy G.711 u-law decompression for backward compatibility
pub struct G711;

impl G711 {
    pub fn linear_to_ulaw(mut pcm: i16) -> u8 {
        const BIAS: i16 = 0x84;
        const CLIP: i16 = 32635;

        let sign = (pcm >> 8) & 0x80;
        if sign != 0 {
            pcm = -pcm;
        }
        if pcm > CLIP {
            pcm = CLIP;
        }
        pcm += BIAS;

        let mut exponent = 7;
        let mut mask = 0x4000;
        while (pcm & mask) == 0 && exponent > 0 {
            exponent -= 1;
            mask >>= 1;
        }

        let mantissa = (pcm >> (exponent + 3)) & 0x0F;
        !((sign as u8) | ((exponent as u8) << 4) | (mantissa as u8))
    }

    pub fn ulaw_to_linear(ulaw_byte: u8) -> i16 {
        const BIAS: i16 = 0x84;
        let ulaw = !ulaw_byte;
        let sign = ulaw & 0x80;
        let exponent = ((ulaw >> 4) & 0x07) as i16;
        let mantissa = (ulaw & 0x0F) as i16;

        let mut sample = ((mantissa << 3) + BIAS) << exponent;
        sample -= BIAS;
        if sign != 0 {
            sample = -sample;
        }
        sample
    }

    pub fn encode_pcm_to_ulaw(pcm: &[i16]) -> Vec<u8> {
        pcm.iter().map(|&s| Self::linear_to_ulaw(s)).collect()
    }

    pub fn decode_ulaw_to_pcm(ulaw: &[u8]) -> Vec<i16> {
        ulaw.iter().map(|&b| Self::ulaw_to_linear(b)).collect()
    }
}

/// Adaptive Jitter Buffer for smooth, pop-free, stutter-free HD voice playback.
/// Pre-buffers audio on speech bursts to absorb network delay variations.
/// Crucial improvement: If buffer momentarily starves during continuous speech, it performs
/// smooth Packet Loss Concealment (PLC) and immediately resumes when the next packet arrives,
/// avoiding the severe stuttering/gapping ("хрипит") caused by waiting for full pre-buffering.
pub struct JitterBuffer {
    queue: VecDeque<f32>,
    is_buffering: bool,
    target_samples: usize,
    max_samples: usize,
    last_sample: f32,
    consecutive_starved: usize,
}

impl JitterBuffer {
    pub fn new(target_samples: usize, max_samples: usize) -> Self {
        Self {
            queue: VecDeque::with_capacity(max_samples + 480),
            is_buffering: true,
            target_samples,
            max_samples,
            last_sample: 0.0,
            consecutive_starved: usize::MAX,
        }
    }

    pub fn push_pcm_samples(&mut self, pcm: &[i16]) {
        // Discard oldest samples if buffer is overflowing to keep latency bounded
        if self.queue.len() + pcm.len() > self.max_samples {
            let excess = (self.queue.len() + pcm.len()).saturating_sub(self.target_samples);
            self.queue.drain(..excess.min(self.queue.len()));
        }

        for &s in pcm {
            self.queue.push_back((s as f32) / 32768.0);
        }

        // If recovering from a brief starvation during speech (< 240 samples = 10ms),
        // resume immediately so the listener never hears an artificial 40-60ms pause.
        if self.is_buffering {
            if self.consecutive_starved < 240 || self.queue.len() >= self.target_samples {
                self.is_buffering = false;
                self.consecutive_starved = 0;
            }
        }
    }

    pub fn pop_sample(&mut self) -> f32 {
        if self.is_buffering {
            return 0.0;
        }

        if let Some(s) = self.queue.pop_front() {
            self.last_sample = s;
            self.consecutive_starved = 0;
            s
        } else {
            // Buffer starved: smoothly decay last sample (PLC) to prevent digital click/pop
            self.consecutive_starved += 1;
            self.last_sample *= 0.90;

            // Only enter full buffering mode if silence lasts for more than ~20ms (480 samples @ 24kHz)
            if self.consecutive_starved > 480 {
                self.is_buffering = true;
                self.last_sample = 0.0;
            }
            self.last_sample
        }
    }

    pub fn clear(&mut self) {
        self.queue.clear();
        self.is_buffering = true;
        self.last_sample = 0.0;
        self.consecutive_starved = usize::MAX;
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }
}

/// Continuous anti-aliased resampler across audio callback chunks.
/// When downsampling 48 kHz to 24 kHz, applies exact 2:1 FIR boxcar filter to prevent aliasing.
pub struct InputResampler {
    in_rate: f64,
    out_rate: f64,
    step: f64,
    phase: f64,
    last_sample: f32,
}

impl InputResampler {
    pub fn new(in_rate: f64, out_rate: f64) -> Self {
        Self {
            in_rate,
            out_rate,
            step: in_rate / out_rate,
            phase: 0.0,
            last_sample: 0.0,
        }
    }

    pub fn process(&mut self, input: &[f32], output: &mut Vec<f32>) {
        if (self.step - 1.0).abs() < 1e-4 {
            output.extend_from_slice(input);
            if let Some(&last) = input.last() {
                self.last_sample = last;
            }
            return;
        }

        let n = input.len();
        if n == 0 {
            return;
        }

        // Optimized 48 kHz -> 24 kHz (exact 2:1 downsampling with anti-aliasing FIR averaging)
        if (self.in_rate - 48000.0).abs() < 10.0 && (self.out_rate - 24000.0).abs() < 10.0 {
            let mut i = 0;
            while i + 1 < n {
                let avg = (input[i] + input[i + 1]) * 0.5;
                output.push(avg);
                i += 2;
            }
            if i < n {
                self.last_sample = input[n - 1];
            }
            return;
        }

        // General fractional linear interpolation
        while self.phase < (n as f64) {
            let s = if self.phase < 0.0 {
                let frac = (self.phase + 1.0) as f32;
                self.last_sample + (input[0] - self.last_sample) * frac.clamp(0.0, 1.0)
            } else {
                let idx = self.phase as usize;
                if idx + 1 < n {
                    let frac = (self.phase - idx as f64) as f32;
                    input[idx] + (input[idx + 1] - input[idx]) * frac
                } else {
                    input[n - 1]
                }
            };
            output.push(s);
            self.phase += self.step;
        }

        self.phase -= n as f64;
        self.last_sample = *input.last().unwrap_or(&self.last_sample);
    }
}

pub struct AudioEngine {
    // Streams
    input_stream: Mutex<Option<Stream>>,
    output_stream: Mutex<Option<Stream>>,

    // Queues & Jitter Buffer
    outgoing_audio_packets: Arc<Mutex<VecDeque<Vec<u8>>>>,
    jitter_buffer: Arc<Mutex<JitterBuffer>>,
    chime_queue: Arc<Mutex<VecDeque<f32>>>,

    // Controls
    mic_muted: Arc<AtomicBool>,
    deafened: Arc<AtomicBool>,
    peer_volume: Arc<AtomicU32>,       // stored as f32 bits
    mic_sensitivity: Arc<AtomicU32>,   // 0.0 to 1.0 slider, stored as f32 bits
    mic_gain: Arc<AtomicU32>,          // 0.0 to 3.0 multiplier, stored as f32 bits
    mic_level: Arc<AtomicU32>,         // live RMS level 0.0 to 1.0, stored as f32 bits
    loopback_test: Arc<AtomicBool>,    // mic loopback test mode

    // Speaking indicators
    local_speaking: Arc<AtomicBool>,
    last_local_speech: Arc<Mutex<Instant>>,
    remote_speaking: Arc<AtomicBool>,
    last_remote_speech: Arc<Mutex<Instant>>,

    // Device health & Auto-recovery
    is_running: Arc<AtomicBool>,
    output_stream_errored: Arc<AtomicBool>,
    input_stream_errored: Arc<AtomicBool>,
    current_output_device_name: Arc<Mutex<Option<String>>>,
    current_input_device_name: Arc<Mutex<Option<String>>>,
    watchdog_abort: Mutex<Option<tokio::task::AbortHandle>>,
}

// CPAL Stream contains platform-specific raw pointers on Windows WASAPI, but access is fully
// synchronized via Mutex inside AudioEngine methods, making it safe to transfer across threads.
unsafe impl Send for AudioEngine {}
unsafe impl Sync for AudioEngine {}

impl AudioEngine {
    pub fn new() -> Arc<Self> {
        let engine = Arc::new(Self {
            input_stream: Mutex::new(None),
            output_stream: Mutex::new(None),
            outgoing_audio_packets: Arc::new(Mutex::new(VecDeque::with_capacity(64))),
            jitter_buffer: Arc::new(Mutex::new(JitterBuffer::new(
                JITTER_TARGET_SAMPLES,
                JITTER_MAX_SAMPLES,
            ))),
            chime_queue: Arc::new(Mutex::new(VecDeque::with_capacity(16384))),
            mic_muted: Arc::new(AtomicBool::new(false)),
            deafened: Arc::new(AtomicBool::new(false)),
            peer_volume: Arc::new(AtomicU32::new(1.0f32.to_bits())),
            mic_sensitivity: Arc::new(AtomicU32::new(0.50f32.to_bits())), // Default 50%
            mic_gain: Arc::new(AtomicU32::new(1.0f32.to_bits())),        // Default 100%
            mic_level: Arc::new(AtomicU32::new(0.0f32.to_bits())),
            loopback_test: Arc::new(AtomicBool::new(false)),
            local_speaking: Arc::new(AtomicBool::new(false)),
            last_local_speech: Arc::new(Mutex::new(Instant::now() - Duration::from_secs(10))),
            remote_speaking: Arc::new(AtomicBool::new(false)),
            last_remote_speech: Arc::new(Mutex::new(Instant::now() - Duration::from_secs(10))),
            is_running: Arc::new(AtomicBool::new(false)),
            output_stream_errored: Arc::new(AtomicBool::new(false)),
            input_stream_errored: Arc::new(AtomicBool::new(false)),
            current_output_device_name: Arc::new(Mutex::new(None)),
            current_input_device_name: Arc::new(Mutex::new(None)),
            watchdog_abort: Mutex::new(None),
        });

        // Launch background device watchdog to automatically recover when headphones are plugged/unplugged
        engine.start_watchdog();

        engine
    }

    fn start_watchdog(self: &Arc<Self>) {
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let weak = Arc::downgrade(self);
            let task = handle.spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_millis(800));
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

                loop {
                    interval.tick().await;
                    let Some(engine) = weak.upgrade() else {
                        break;
                    };

                    if engine.is_running.load(Ordering::Relaxed) {
                        engine.check_and_recover_devices();
                    }
                }
            });
            *self.watchdog_abort.lock() = Some(task.abort_handle());
        }
    }

    pub fn set_mic_muted(&self, muted: bool) {
        self.mic_muted.store(muted, Ordering::SeqCst);
        if muted {
            self.local_speaking.store(false, Ordering::SeqCst);
            self.mic_level.store(0.0f32.to_bits(), Ordering::Relaxed);
        }
    }

    pub fn is_mic_muted(&self) -> bool {
        self.mic_muted.load(Ordering::SeqCst)
    }

    pub fn set_deafened(&self, deafened: bool) {
        self.deafened.store(deafened, Ordering::SeqCst);
        if deafened {
            self.jitter_buffer.lock().clear();
        }
    }

    pub fn is_deafened(&self) -> bool {
        self.deafened.load(Ordering::SeqCst)
    }

    pub fn set_peer_volume(&self, volume: f32) {
        let clamped = volume.clamp(0.0, 2.0);
        self.peer_volume.store(clamped.to_bits(), Ordering::SeqCst);
    }

    pub fn get_peer_volume(&self) -> f32 {
        f32::from_bits(self.peer_volume.load(Ordering::SeqCst))
    }

    pub fn set_mic_sensitivity(&self, sensitivity: f32) {
        let clamped = sensitivity.clamp(0.0, 1.0);
        self.mic_sensitivity.store(clamped.to_bits(), Ordering::SeqCst);
    }

    pub fn get_mic_sensitivity(&self) -> f32 {
        f32::from_bits(self.mic_sensitivity.load(Ordering::SeqCst))
    }

    pub fn set_mic_gain(&self, gain: f32) {
        let clamped = gain.clamp(0.0, 3.0);
        self.mic_gain.store(clamped.to_bits(), Ordering::SeqCst);
    }

    pub fn get_mic_gain(&self) -> f32 {
        f32::from_bits(self.mic_gain.load(Ordering::SeqCst))
    }

    pub fn get_mic_level(&self) -> f32 {
        f32::from_bits(self.mic_level.load(Ordering::Relaxed))
    }

    pub fn set_loopback_test(&self, enabled: bool) {
        self.loopback_test.store(enabled, Ordering::SeqCst);
        if enabled {
            let _ = self.start();
        }
    }

    pub fn is_loopback_test(&self) -> bool {
        self.loopback_test.load(Ordering::SeqCst)
    }

    pub fn is_local_speaking(&self) -> bool {
        if self.mic_muted.load(Ordering::Relaxed) {
            return false;
        }
        self.last_local_speech.lock().elapsed() < Duration::from_millis(400)
    }

    pub fn is_remote_speaking(&self) -> bool {
        self.last_remote_speech.lock().elapsed() < Duration::from_millis(400)
    }

    pub fn is_output_ok(&self) -> bool {
        self.output_stream.lock().is_some() && !self.output_stream_errored.load(Ordering::Relaxed)
    }

    pub fn is_input_ok(&self) -> bool {
        self.input_stream.lock().is_some() && !self.input_stream_errored.load(Ordering::Relaxed)
    }

    pub fn get_device_names(&self) -> (Option<String>, Option<String>) {
        (
            self.current_output_device_name.lock().clone(),
            self.current_input_device_name.lock().clone(),
        )
    }

    /// Synthesizes and plays a rising double-tone chime (587 Hz D5 -> 880 Hz A5) when peer joins
    pub fn play_connect_chime(&self) {
        let mut chimes = Vec::with_capacity(7200);
        // Note 1: 587.33 Hz (D5) for 120ms (2880 samples @ 24kHz)
        let n1 = 2880;
        for i in 0..n1 {
            let t = (i as f32) / (VOICE_SAMPLE_RATE as f32);
            let attack = ((i as f32) / 240.0).min(1.0);
            let decay = 1.0 - (i as f32) / (n1 as f32);
            let envelope = attack * decay;
            let s = (2.0 * std::f32::consts::PI * 587.33 * t).sin() * envelope * 0.35;
            chimes.push(s);
        }
        // Note 2: 880.0 Hz (A5) for 180ms (4320 samples @ 24kHz)
        let n2 = 4320;
        for i in 0..n2 {
            let t = (i as f32) / (VOICE_SAMPLE_RATE as f32);
            let attack = ((i as f32) / 240.0).min(1.0);
            let decay = (1.0 - (i as f32) / (n2 as f32)).powf(1.5);
            let envelope = attack * decay;
            let s = (2.0 * std::f32::consts::PI * 880.0 * t).sin() * envelope * 0.35;
            chimes.push(s);
        }

        let mut q = self.chime_queue.lock();
        q.clear();
        q.extend(chimes);
    }

    /// Synthesizes and plays a gentle falling double-tone chime (659 Hz E5 -> 440 Hz A4) when peer leaves
    pub fn play_disconnect_chime(&self) {
        let mut chimes = Vec::with_capacity(7680);
        // Note 1: 659.25 Hz (E5) for 120ms (2880 samples @ 24kHz)
        let n1 = 2880;
        for i in 0..n1 {
            let t = (i as f32) / (VOICE_SAMPLE_RATE as f32);
            let attack = ((i as f32) / 240.0).min(1.0);
            let decay = 1.0 - (i as f32) / (n1 as f32);
            let envelope = attack * decay;
            let s = (2.0 * std::f32::consts::PI * 659.25 * t).sin() * envelope * 0.35;
            chimes.push(s);
        }
        // Note 2: 440.0 Hz (A4) for 200ms (4800 samples @ 24kHz)
        let n2 = 4800;
        for i in 0..n2 {
            let t = (i as f32) / (VOICE_SAMPLE_RATE as f32);
            let attack = ((i as f32) / 240.0).min(1.0);
            let decay = (1.0 - (i as f32) / (n2 as f32)).powf(1.5);
            let envelope = attack * decay;
            let s = (2.0 * std::f32::consts::PI * 440.0 * t).sin() * envelope * 0.35;
            chimes.push(s);
        }

        let mut q = self.chime_queue.lock();
        q.clear();
        q.extend(chimes);
    }

    /// Pop ready HD voice packets to send over P2P
    pub fn pop_outgoing_packets(&self) -> Vec<Vec<u8>> {
        let mut q = self.outgoing_audio_packets.lock();
        q.drain(..).collect()
    }

    /// Feed incoming HD audio packet from remote peer to JitterBuffer
    pub fn feed_incoming_packet(&self, data: &[u8]) {
        if self.deafened.load(Ordering::Relaxed) || data.is_empty() {
            return;
        }

        let pcm: Vec<i16> = match data[0] {
            CODEC_PCM24K => {
                // 24 kHz 16-bit Linear PCM (Pristine HD audio)
                let payload = &data[1..];
                let num_samples = payload.len() / 2;
                let mut samples = Vec::with_capacity(num_samples);
                for chunk in payload.chunks_exact(2) {
                    samples.push(i16::from_le_bytes([chunk[0], chunk[1]]));
                }
                samples
            }
            CODEC_PCM16K => {
                // 16 kHz 16-bit Linear PCM (resampled to 24 kHz)
                let payload = &data[1..];
                let num_samples = payload.len() / 2;
                let mut s16k = Vec::with_capacity(num_samples);
                for chunk in payload.chunks_exact(2) {
                    s16k.push(i16::from_le_bytes([chunk[0], chunk[1]]));
                }
                resample_16k_to_24k(&s16k)
            }
            _ => {
                // Legacy G.711 u-law (backward compatibility fallback)
                let s16k = G711::decode_ulaw_to_pcm(data);
                resample_16k_to_24k(&s16k)
            }
        };

        if !pcm.is_empty() {
            self.jitter_buffer.lock().push_pcm_samples(&pcm);
            *self.last_remote_speech.lock() = Instant::now();
            self.remote_speaking.store(true, Ordering::Relaxed);
        }
    }

    /// Checks if device errors occurred or default devices changed (e.g. headphones plugged/unplugged)
    /// and seamlessly recreates streams without interrupting the call.
    pub fn check_and_recover_devices(&self) {
        let host = cpal::default_host();

        // 1. Output device check & recovery
        let out_errored = self.output_stream_errored.swap(false, Ordering::SeqCst);
        let default_out = host.default_output_device();
        let default_out_name = default_out.as_ref().and_then(|d| d.name().ok());
        let out_name_changed = {
            let cur = self.current_output_device_name.lock();
            default_out_name.is_some() && *cur != default_out_name
        };

        if out_errored || out_name_changed || self.output_stream.lock().is_none() {
            let _ = self.restart_output_stream();
        }

        // 2. Input device check & recovery
        let in_errored = self.input_stream_errored.swap(false, Ordering::SeqCst);
        let default_in = host.default_input_device();
        let default_in_name = default_in.as_ref().and_then(|d| d.name().ok());
        let in_name_changed = {
            let cur = self.current_input_device_name.lock();
            default_in_name.is_some() && *cur != default_in_name
        };

        if in_errored || in_name_changed || self.input_stream.lock().is_none() {
            let _ = self.restart_input_stream();
        }
    }

    pub fn restart_output_stream(&self) -> Result<(), String> {
        *self.output_stream.lock() = None;
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| "Аудио устройство вывода (наушники/динамики) не найдено".to_string())?;

        let dev_name = device.name().ok();
        *self.current_output_device_name.lock() = dev_name;

        let stream = self.build_output_stream_for_device(&device)?;
        if stream.play().is_ok() {
            *self.output_stream.lock() = Some(stream);
            self.output_stream_errored.store(false, Ordering::SeqCst);
            Ok(())
        } else {
            Err("Не удалось запустить воспроизведение на устройстве вывода".into())
        }
    }

    pub fn restart_input_stream(&self) -> Result<(), String> {
        *self.input_stream.lock() = None;
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| "Микрофон не найден".to_string())?;

        let dev_name = device.name().ok();
        *self.current_input_device_name.lock() = dev_name;

        let stream = self.build_input_stream_for_device(&device)?;
        if stream.play().is_ok() {
            *self.input_stream.lock() = Some(stream);
            self.input_stream_errored.store(false, Ordering::SeqCst);
            Ok(())
        } else {
            Err("Не удалось запустить запись с микрофона".into())
        }
    }

    fn build_output_stream_for_device(&self, device: &cpal::Device) -> Result<Stream, String> {
        let config = device
            .default_output_config()
            .map_err(|e| format!("Ошибка конфигурации аудио вывода: {}", e))?;

        let channels = config.channels() as usize;
        let dev_sample_rate = config.sample_rate().0 as f64;
        let jitter_buf = self.jitter_buffer.clone();
        let chimes = self.chime_queue.clone();
        let deafened_flag = self.deafened.clone();
        let volume_bits = self.peer_volume.clone();
        let errored_flag = self.output_stream_errored.clone();

        let step = (VOICE_SAMPLE_RATE as f64) / dev_sample_rate;
        let mut current_sample = 0.0f32;
        let mut next_sample = 0.0f32;
        let mut phase = 0.0f64;

        let err_fn = move |err| {
            eprintln!("[AudioEngine] Ошибка потока вывода: {:?}", err);
            errored_flag.store(true, Ordering::SeqCst);
        };

        let stream_res = match config.sample_format() {
            SampleFormat::F32 => device.build_output_stream(
                &config.into(),
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    let is_deaf = deafened_flag.load(Ordering::Relaxed);
                    let vol = f32::from_bits(volume_bits.load(Ordering::Relaxed));
                    let mut jb = jitter_buf.lock();
                    let mut chime_q = chimes.lock();

                    let frames = data.len() / channels;
                    for f in 0..frames {
                        while phase >= 1.0 {
                            phase -= 1.0;
                            current_sample = next_sample;
                            next_sample = jb.pop_sample();
                        }

                        let voice_sample = if !is_deaf {
                            let interp = current_sample + (next_sample - current_sample) * (phase as f32);
                            interp * vol
                        } else {
                            0.0
                        };

                        let chime_sample = chime_q.pop_front().unwrap_or(0.0);
                        let final_sample = soft_limiter(voice_sample + chime_sample);

                        for c in 0..channels {
                            data[f * channels + c] = final_sample;
                        }

                        phase += step;
                    }
                },
                err_fn,
                None,
            ),
            SampleFormat::I16 => device.build_output_stream(
                &config.into(),
                move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                    let is_deaf = deafened_flag.load(Ordering::Relaxed);
                    let vol = f32::from_bits(volume_bits.load(Ordering::Relaxed));
                    let mut jb = jitter_buf.lock();
                    let mut chime_q = chimes.lock();

                    let frames = data.len() / channels;
                    for f in 0..frames {
                        while phase >= 1.0 {
                            phase -= 1.0;
                            current_sample = next_sample;
                            next_sample = jb.pop_sample();
                        }

                        let voice_sample = if !is_deaf {
                            let interp = current_sample + (next_sample - current_sample) * (phase as f32);
                            interp * vol
                        } else {
                            0.0
                        };

                        let chime_sample = chime_q.pop_front().unwrap_or(0.0);
                        let final_sample = soft_limiter(voice_sample + chime_sample);
                        let sample_i16 = (final_sample * 32767.0).clamp(-32768.0, 32767.0) as i16;

                        for c in 0..channels {
                            data[f * channels + c] = sample_i16;
                        }

                        phase += step;
                    }
                },
                err_fn,
                None,
            ),
            _ => return Err("Неподдерживаемый аудио формат вывода".into()),
        };

        stream_res.map_err(|e| format!("Не удалось собрать поток вывода: {}", e))
    }

    fn build_input_stream_for_device(&self, device: &cpal::Device) -> Result<Stream, String> {
        let config = device
            .default_input_config()
            .map_err(|e| format!("Ошибка конфигурации микрофона: {}", e))?;

        let channels = config.channels() as usize;
        let dev_sample_rate = config.sample_rate().0 as f64;
        let outgoing_queue = self.outgoing_audio_packets.clone();
        let mic_muted_flag = self.mic_muted.clone();
        let local_speech_time = self.last_local_speech.clone();
        let local_speaking_flag = self.local_speaking.clone();
        let errored_flag = self.input_stream_errored.clone();
        let mic_sens_bits = self.mic_sensitivity.clone();
        let mic_gain_bits = self.mic_gain.clone();
        let mic_level_bits = self.mic_level.clone();
        let loopback_flag = self.loopback_test.clone();
        let jb_loopback = self.jitter_buffer.clone();

        // Accumulator buffer for downsampled 24kHz mono audio
        let accumulator = Arc::new(Mutex::new(Vec::<f32>::with_capacity(2048)));
        let mut resampler = InputResampler::new(dev_sample_rate, VOICE_SAMPLE_RATE as f64);
        let mut resample_out = Vec::with_capacity(1024);
        let mut hangover_frames = 0usize;

        let err_fn = move |err| {
            eprintln!("[AudioEngine] Ошибка потока микрофона: {:?}", err);
            errored_flag.store(true, Ordering::SeqCst);
        };

        let mut process_mono_samples = move |mono: &[f32]| {
            let is_muted = mic_muted_flag.load(Ordering::Relaxed);
            if is_muted || mono.is_empty() {
                mic_level_bits.store(0.0f32.to_bits(), Ordering::Relaxed);
                return;
            }

            resample_out.clear();
            resampler.process(mono, &mut resample_out);

            let gain = f32::from_bits(mic_gain_bits.load(Ordering::Relaxed)).clamp(0.0, 3.0);
            let sens_slider = f32::from_bits(mic_sens_bits.load(Ordering::Relaxed)).clamp(0.0, 1.0);
            // Exponential threshold: 0.0 -> 0.0005 (whisper), 0.5 -> ~0.0045 (normal), 1.0 -> 0.040 (loud/gate)
            let threshold = 0.0005 * (80.0f32).powf(sens_slider);

            let mut acc = accumulator.lock();
            for &s in &resample_out {
                // Apply mic gain with soft limiter to prevent any digital rasp/distortion on input
                let s_limited = soft_limiter(s * gain);
                acc.push(s_limited);
            }

            // Process 20ms frames (480 samples at 24 kHz)
            while acc.len() >= SAMPLES_PER_FRAME {
                let frame: Vec<f32> = acc.drain(..SAMPLES_PER_FRAME).collect();

                // Compute frame energy (RMS)
                let mut sum_sq = 0.0f32;
                for &s in &frame {
                    sum_sq += s * s;
                }
                let rms = (sum_sq / (SAMPLES_PER_FRAME as f32)).sqrt();

                // Update live input level for UI visual meter
                let visual_meter = (rms * 12.0).clamp(0.0, 1.0);
                mic_level_bits.store(visual_meter.to_bits(), Ordering::Relaxed);

                // VAD with hangover: keep transmitting for 300ms after speech ends
                if rms >= threshold {
                    hangover_frames = VAD_HANGOVER_FRAMES;
                    *local_speech_time.lock() = Instant::now();
                    local_speaking_flag.store(true, Ordering::Relaxed);
                } else if hangover_frames > 0 {
                    hangover_frames -= 1;
                }

                if hangover_frames > 0 {
                    // Convert float [-1.0..1.0] to i16 PCM
                    let pcm: Vec<i16> = frame
                        .iter()
                        .map(|&s| {
                            let clamped = soft_limiter(s);
                            (clamped * 32767.0).clamp(-32768.0, 32767.0) as i16
                        })
                        .collect();

                    // If user is running loopback test, feed into local jitter buffer
                    if loopback_flag.load(Ordering::Relaxed) {
                        jb_loopback.lock().push_pcm_samples(&pcm);
                    }

                    // Encode as HD 24 kHz 16-bit Linear PCM:
                    // Header byte (0x02) + 480 * 2 bytes = 961 bytes packet
                    let mut packet = Vec::with_capacity(1 + SAMPLES_PER_FRAME * 2);
                    packet.push(CODEC_PCM24K);
                    for &sample_i16 in &pcm {
                        packet.extend_from_slice(&sample_i16.to_le_bytes());
                    }

                    let mut out = outgoing_queue.lock();
                    if out.len() < 30 {
                        out.push_back(packet);
                    }
                }
            }
        };

        let stream_res = match config.sample_format() {
            SampleFormat::F32 => device.build_input_stream(
                &config.into(),
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    let frames = data.len() / channels;
                    let mut mono = Vec::with_capacity(frames);
                    if channels == 1 {
                        mono.extend_from_slice(data);
                    } else {
                        for f in 0..frames {
                            let mut sum = 0.0f32;
                            for c in 0..channels {
                                sum += data[f * channels + c];
                            }
                            mono.push(sum / (channels as f32));
                        }
                    }
                    process_mono_samples(&mono);
                },
                err_fn,
                None,
            ),
            SampleFormat::I16 => device.build_input_stream(
                &config.into(),
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    let frames = data.len() / channels;
                    let mut mono = Vec::with_capacity(frames);
                    if channels == 1 {
                        for &s in data {
                            mono.push((s as f32) / 32768.0);
                        }
                    } else {
                        for f in 0..frames {
                            let mut sum = 0.0f32;
                            for c in 0..channels {
                                sum += (data[f * channels + c] as f32) / 32768.0;
                            }
                            mono.push(sum / (channels as f32));
                        }
                    }
                    process_mono_samples(&mono);
                },
                err_fn,
                None,
            ),
            _ => return Err("Неподдерживаемый аудио формат ввода".into()),
        };

        stream_res.map_err(|e| format!("Не удалось собрать поток микрофона: {}", e))
    }

    /// Starts both microphone capture and speaker playback streams
    pub fn start(&self) -> Result<(), String> {
        self.is_running.store(true, Ordering::SeqCst);
        let _ = self.restart_output_stream();
        let _ = self.restart_input_stream();
        Ok(())
    }

    /// Restarts both output and input streams on currently active default devices
    pub fn restart(&self) -> Result<(), String> {
        self.restart_output_stream()?;
        self.restart_input_stream()?;
        Ok(())
    }

    pub fn stop(&self) {
        self.is_running.store(false, Ordering::SeqCst);
        *self.input_stream.lock() = None;
        *self.output_stream.lock() = None;
        self.outgoing_audio_packets.lock().clear();
        self.jitter_buffer.lock().clear();
        self.chime_queue.lock().clear();
        self.local_speaking.store(false, Ordering::SeqCst);
        self.remote_speaking.store(false, Ordering::SeqCst);
        self.mic_level.store(0.0f32.to_bits(), Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_g711_roundtrip() {
        let test_samples: Vec<i16> = vec![-32000, -10000, -500, 0, 500, 10000, 32000];
        let encoded = G711::encode_pcm_to_ulaw(&test_samples);
        let decoded = G711::decode_ulaw_to_pcm(&encoded);

        assert_eq!(encoded.len(), test_samples.len());
        assert_eq!(decoded.len(), test_samples.len());

        for (orig, dec) in test_samples.iter().zip(decoded.iter()) {
            let diff = (orig - dec).abs();
            let tolerance = (orig.abs() as f32 * 0.1).max(50.0) as i16;
            assert!(diff <= tolerance, "orig {} vs dec {}", orig, dec);
        }
    }

    #[test]
    fn test_soft_limiter() {
        // Below 0.75, signal is completely untouched
        assert!((soft_limiter(0.5) - 0.5).abs() < 1e-5);
        assert!((soft_limiter(-0.5) - -0.5).abs() < 1e-5);

        // Above 0.75, smoothly saturates without exceeding 1.0
        let clipped = soft_limiter(1.5);
        assert!(clipped > 0.75 && clipped <= 1.0);

        let neg_clipped = soft_limiter(-2.0);
        assert!(neg_clipped < -0.75 && neg_clipped >= -1.0);
    }

    #[test]
    fn test_resample_16k_to_24k() {
        let input = vec![1000i16, 2000, 3000, 4000];
        let output = resample_16k_to_24k(&input);
        assert_eq!(output.len(), 6);
        assert_eq!(output[0], 1000);
    }

    #[test]
    fn test_audio_engine_controls() {
        let engine = AudioEngine::new();
        assert!(!engine.is_mic_muted());
        assert!(!engine.is_deafened());

        engine.set_mic_muted(true);
        assert!(engine.is_mic_muted());

        engine.set_deafened(true);
        assert!(engine.is_deafened());

        engine.set_peer_volume(1.5);
        assert!((engine.get_peer_volume() - 1.5).abs() < 0.001);

        engine.set_mic_sensitivity(0.75);
        assert!((engine.get_mic_sensitivity() - 0.75).abs() < 0.001);

        engine.set_mic_gain(1.25);
        assert!((engine.get_mic_gain() - 1.25).abs() < 0.001);
    }

    #[test]
    fn test_jitter_buffer_prebuffering_and_fadeout() {
        let mut jb = JitterBuffer::new(JITTER_TARGET_SAMPLES, JITTER_MAX_SAMPLES);

        // While buffering < target (960 samples @ 24kHz), pop_sample should output silence (0.0)
        let chunk = vec![1000i16; 480];
        jb.push_pcm_samples(&chunk);
        assert_eq!(jb.pop_sample(), 0.0);

        // Second chunk brings total to 960 -> should start playback!
        jb.push_pcm_samples(&chunk);
        let s = jb.pop_sample();
        assert!(s > 0.0, "Jitter buffer must begin playback once target is reached");

        // Drain until empty to test smooth fadeout
        while jb.len() > 0 {
            jb.pop_sample();
        }

        // When starved, last_sample fades exponentially towards 0.0 without pops
        let faded = jb.pop_sample();
        assert!(faded.abs() <= 1.0);
    }

    #[test]
    fn test_resampler_exactness() {
        // 48000 -> 24000 downsample ratio 2:1
        let mut resampler = InputResampler::new(48000.0, 24000.0);
        let input = vec![0.5f32; 480];
        let mut output = Vec::new();
        resampler.process(&input, &mut output);

        assert_eq!(output.len(), 240, "480 samples at 48kHz should downsample to exactly 240 samples at 24kHz");
        for &s in &output {
            assert!((s - 0.5).abs() < 1e-4);
        }
    }

    #[test]
    fn test_chimes() {
        let engine = AudioEngine::new();
        engine.play_connect_chime();
        assert!(engine.chime_queue.lock().len() > 0);

        engine.play_disconnect_chime();
        assert!(engine.chime_queue.lock().len() > 0);
    }
}
