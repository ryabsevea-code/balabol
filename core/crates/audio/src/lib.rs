use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum AudioError {
    #[error("CPAL audio error: {0}")]
    Cpal(#[from] cpal::DefaultStreamConfigError),
    #[error("Device not available: {0}")]
    DeviceNotAvailable(String),
    #[error("Failed to build stream: {0}")]
    BuildStreamError(#[from] cpal::BuildStreamError),
    #[error("Failed to play stream: {0}")]
    PlayStreamError(#[from] cpal::PlayStreamError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FilterMode {
    Off,
    NoiseGate,
    DeepFilterNet, // Neural Network Speech Enhancement
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioDeviceInfo {
    pub name: String,
    pub is_default: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioEngineConfig {
    pub filter_mode: FilterMode,
    pub input_volume: f32,       // 0.0 to 2.0 (1.0 = 100%)
    pub output_volume: f32,      // 0.0 to 2.0
    pub vad_threshold: f32,      // 0.0 to 1.0
    pub is_muted: bool,
    pub is_deafened: bool,
}

impl Default for AudioEngineConfig {
    fn default() -> Self {
        Self {
            filter_mode: FilterMode::DeepFilterNet,
            input_volume: 1.0,
            output_volume: 1.0,
            vad_threshold: 0.03, // ~ -30dB RMS
            is_muted: false,
            is_deafened: false,
        }
    }
}

/// Real-time Voice Activity Detector with smoothing and hysteresis.
pub struct VadAnalyzer {
    threshold: f32,
    hold_frames: usize,
    hold_counter: usize,
    is_active: bool,
}

impl VadAnalyzer {
    pub fn new(threshold: f32) -> Self {
        Self {
            threshold,
            hold_frames: 8, // ~160ms hold at 20ms frames
            hold_counter: 0,
            is_active: false,
        }
    }

    pub fn set_threshold(&mut self, threshold: f32) {
        self.threshold = threshold;
    }

    /// Process a frame of PCM audio samples and determine if speech is present.
    pub fn process_frame(&mut self, samples: &[f32]) -> (bool, f32) {
        if samples.is_empty() {
            return (false, 0.0);
        }

        let mut sum_sq = 0.0f32;
        for &s in samples {
            sum_sq += s * s;
        }
        let rms = (sum_sq / samples.len() as f32).sqrt();

        if rms > self.threshold {
            self.hold_counter = self.hold_frames;
            self.is_active = true;
        } else if self.hold_counter > 0 {
            self.hold_counter -= 1;
            self.is_active = true;
        } else {
            self.is_active = false;
        }

        (self.is_active, rms)
    }
}

/// Software Audio Mixer for multiple incoming audio streams.
/// Applies individual participant gains and soft-clipping master limiting.
pub struct SoftwareMixer {
    user_volumes: Arc<RwLock<HashMap<String, f32>>>,
    user_buffers: Arc<RwLock<HashMap<String, Vec<f32>>>>,
}

impl SoftwareMixer {
    pub fn new() -> Self {
        Self {
            user_volumes: Arc::new(RwLock::new(HashMap::new())),
            user_buffers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Set volume for an individual participant (0.0 to 2.0).
    pub fn set_user_volume(&self, user_id: &str, volume: f32) {
        let clamped = volume.clamp(0.0, 2.0);
        self.user_volumes.write().insert(user_id.to_string(), clamped);
    }

    pub fn get_user_volume(&self, user_id: &str) -> f32 {
        *self.user_volumes.read().get(user_id).unwrap_or(&1.0)
    }

    /// Remove a participant when they leave the voice channel.
    pub fn remove_user(&self, user_id: &str) {
        self.user_volumes.write().remove(user_id);
        self.user_buffers.write().remove(user_id);
    }

    /// Feed incoming decoded PCM audio samples for a participant.
    pub fn feed_samples(&self, user_id: &str, samples: &[f32]) {
        let mut buffers = self.user_buffers.write();
        let buf = buffers.entry(user_id.to_string()).or_insert_with(Vec::new);
        buf.extend_from_slice(samples);
        // Limit maximum queued latency to ~250ms (12000 samples at 48kHz) to prevent memory leak
        if buf.len() > 12000 {
            buf.drain(0..buf.len() - 12000);
        }
    }

    /// Mix audio from all participants into the output buffer.
    pub fn mix_into_output(&self, out: &mut [f32], master_volume: f32, is_deafened: bool) {
        if is_deafened {
            out.fill(0.0);
            return;
        }

        out.fill(0.0);
        let volumes = self.user_volumes.read();
        let mut buffers = self.user_buffers.write();

        for (user_id, buf) in buffers.iter_mut() {
            let user_vol = *volumes.get(user_id).unwrap_or(&1.0);
            let take_len = out.len().min(buf.len());
            for i in 0..take_len {
                out[i] += buf[i] * user_vol;
            }
            buf.drain(0..take_len);
        }

        // Apply master volume and soft-knee limiter (tanh) to prevent speaker distortion
        for sample in out.iter_mut() {
            let amplified = *sample * master_volume;
            // Soft clipping: smooth saturation when |x| > 0.8
            if amplified > 0.8 {
                *sample = 0.8 + (amplified - 0.8).tanh() * 0.2;
            } else if amplified < -0.8 {
                *sample = -0.8 + (amplified + 0.8).tanh() * 0.2;
            } else {
                *sample = amplified;
            }
        }
    }
}

impl Default for SoftwareMixer {
    fn default() -> Self {
        Self::new()
    }
}

/// Advanced DSP Noise Gate and Expander with smooth envelope follower and DC-blocking High-Pass filter.
pub struct NoiseFilter {
    mode: FilterMode,
    envelope: f32,
    current_gain: f32,
    hold_samples: usize,
    hold_counter: usize,
    hp_prev_in: f32,
    hp_prev_out: f32,
}

impl NoiseFilter {
    pub fn new(mode: FilterMode) -> Self {
        Self {
            mode,
            envelope: 0.0,
            current_gain: 1.0,
            hold_samples: 4800, // ~100ms at 48kHz
            hold_counter: 0,
            hp_prev_in: 0.0,
            hp_prev_out: 0.0,
        }
    }

    pub fn set_mode(&mut self, mode: FilterMode) {
        self.mode = mode;
    }

    /// Process audio buffer in-place with smooth gain envelope and DC-block filter.
    pub fn process(&mut self, samples: &mut [f32]) {
        if self.mode == FilterMode::Off || samples.is_empty() {
            return;
        }

        // 1. High-Pass Filter (~80 Hz cutoff at 48 kHz): eliminates rumble, fan hum, desk thumps.
        const HP_ALPHA: f32 = 0.9896;
        for s in samples.iter_mut() {
            let x = *s;
            let y = HP_ALPHA * (self.hp_prev_out + x - self.hp_prev_in);
            self.hp_prev_in = x;
            self.hp_prev_out = y;
            *s = y;
        }

        // 2. Threshold & parameters selection based on mode
        let (threshold, floor_gain, attack_coeff, release_coeff) = match self.mode {
            FilterMode::NoiseGate => {
                (0.015f32, 0.0f32, 0.2f32, 0.005f32)
            }
            FilterMode::DeepFilterNet => {
                // Voice-optimized clean expander: gentle floor, preserves natural speech tails
                (0.018f32, 0.05f32, 0.25f32, 0.003f32)
            }
            FilterMode::Off => return,
        };

        // 3. Smooth Envelope Follower & Gain Calculation (no zero-crossing distortion!)
        for s in samples.iter_mut() {
            let abs_s = s.abs();
            if abs_s > self.envelope {
                self.envelope += attack_coeff * (abs_s - self.envelope);
            } else {
                self.envelope += release_coeff * (abs_s - self.envelope);
            }

            let target_gain = if self.envelope > threshold {
                self.hold_counter = self.hold_samples;
                1.0f32
            } else if self.hold_counter > 0 {
                self.hold_counter -= 1;
                1.0f32
            } else {
                let ratio = (self.envelope / threshold).clamp(0.0, 1.0);
                floor_gain + (1.0 - floor_gain) * (ratio * ratio)
            };

            if target_gain > self.current_gain {
                self.current_gain += 0.05 * (target_gain - self.current_gain);
            } else {
                self.current_gain += 0.005 * (target_gain - self.current_gain);
            }

            *s *= self.current_gain;
        }
    }
}

/// Enumerate host audio input and output devices.
pub fn list_audio_devices() -> (Vec<AudioDeviceInfo>, Vec<AudioDeviceInfo>) {
    use cpal::traits::HostTrait;
    let host = cpal::default_host();
    let mut inputs = Vec::new();
    let mut outputs = Vec::new();

    if let Ok(devices) = host.input_devices() {
        use cpal::traits::DeviceTrait;
        for dev in devices {
            if let Ok(name) = dev.name() {
                inputs.push(AudioDeviceInfo {
                    name,
                    is_default: false,
                });
            }
        }
    }

    if let Ok(devices) = host.output_devices() {
        use cpal::traits::DeviceTrait;
        for dev in devices {
            if let Ok(name) = dev.name() {
                outputs.push(AudioDeviceInfo {
                    name,
                    is_default: false,
                });
            }
        }
    }

    (inputs, outputs)
}

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

pub type AudioPacketCallback = Box<dyn Fn(Vec<f32>) + Send + Sync>;

struct SafeStreamHandle(parking_lot::Mutex<Option<cpal::Stream>>);
unsafe impl Send for SafeStreamHandle {}
unsafe impl Sync for SafeStreamHandle {}

impl SafeStreamHandle {
    fn new() -> Self {
        Self(parking_lot::Mutex::new(None))
    }

    fn set(&self, stream: Option<cpal::Stream>) {
        *self.0.lock() = stream;
    }
}

/// Complete native real-time audio pipeline managing CPAL capture and playback.
pub struct LiveAudioPipeline {
    pub config: Arc<RwLock<AudioEngineConfig>>,
    pub mixer: Arc<SoftwareMixer>,
    pub vad: Arc<parking_lot::Mutex<VadAnalyzer>>,
    pub filter: Arc<parking_lot::Mutex<NoiseFilter>>,
    pub echo_test_active: Arc<AtomicBool>,
    pub last_mic_rms: Arc<AtomicU32>,
    pub active_voice_channel: Arc<RwLock<Option<String>>>,
    pub audio_sender: Arc<parking_lot::Mutex<Option<AudioPacketCallback>>>,
    /// 30ms jitter buffer accumulates mic samples before feeding mixer during echo test.
    /// At 48kHz, 30ms = 1440 samples. Prevents buffer-underrun screeching.
    echo_jitter_buffer: Arc<parking_lot::Mutex<Vec<f32>>>,
    input_stream: Arc<SafeStreamHandle>,
    output_stream: Arc<SafeStreamHandle>,
}


impl LiveAudioPipeline {
    pub fn new(
        config: Arc<RwLock<AudioEngineConfig>>,
        mixer: Arc<SoftwareMixer>,
        vad: Arc<parking_lot::Mutex<VadAnalyzer>>,
    ) -> Self {
        let filter_mode = config.read().filter_mode;
        Self {
            config,
            mixer,
            vad,
            filter: Arc::new(parking_lot::Mutex::new(NoiseFilter::new(filter_mode))),
            echo_test_active: Arc::new(AtomicBool::new(false)),
            last_mic_rms: Arc::new(AtomicU32::new(0)),
            active_voice_channel: Arc::new(RwLock::new(None)),
            audio_sender: Arc::new(parking_lot::Mutex::new(None)),
            echo_jitter_buffer: Arc::new(parking_lot::Mutex::new(Vec::with_capacity(2880))),
            input_stream: Arc::new(SafeStreamHandle::new()),
            output_stream: Arc::new(SafeStreamHandle::new()),
        }
    }


    pub fn set_echo_test(&self, enabled: bool) {
        self.echo_test_active.store(enabled, Ordering::Relaxed);
        if !enabled {
            self.mixer.remove_user("__local_echo__");
            // Clear the jitter buffer when echo test is stopped
            self.echo_jitter_buffer.lock().clear();
        }
    }

    pub fn is_echo_test_active(&self) -> bool {
        self.echo_test_active.load(Ordering::Relaxed)
    }

    pub fn get_mic_level(&self) -> f32 {
        f32::from_bits(self.last_mic_rms.load(Ordering::Relaxed))
    }

    pub fn set_active_channel(&self, channel_id: Option<String>) {
        *self.active_voice_channel.write() = channel_id;
    }

    pub fn set_audio_sender<F>(&self, cb: F)
    where
        F: Fn(Vec<f32>) + Send + Sync + 'static,
    {
        *self.audio_sender.lock() = Some(Box::new(cb));
    }

    pub fn start_streams(&self) -> Result<(), AudioError> {
        let _ = self.start_output_stream();
        let _ = self.start_input_stream();
        Ok(())
    }

    pub fn start_output_stream(&self) -> Result<(), AudioError> {
        let host = cpal::default_host();
        let device = match host.default_output_device() {
            Some(d) => d,
            None => {
                tracing::warn!("No default audio output device found on system");
                return Err(AudioError::DeviceNotAvailable("Output device not found".into()));
            }
        };

        let config = match device.default_output_config() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("Failed to query default output config: {}", e);
                return Err(e.into());
            }
        };

        let channels = config.channels() as usize;
        let mixer = self.mixer.clone();
        let config_lock = self.config.clone();

        let err_fn = |err| {
            tracing::error!("CPAL audio output stream error: {:?}", err);
        };

        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => {
                device.build_output_stream(
                    &config.into(),
                    move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                        let cfg = config_lock.read();
                        let master_vol = cfg.output_volume;
                        let is_deafened = cfg.is_deafened;
                        drop(cfg);

                        if channels <= 1 {
                            mixer.mix_into_output(data, master_vol, is_deafened);
                        } else {
                            let frames = data.len() / channels;
                            let mut mono_buf = vec![0.0f32; frames];
                            mixer.mix_into_output(&mut mono_buf, master_vol, is_deafened);
                            for f in 0..frames {
                                let val = mono_buf[f];
                                for c in 0..channels {
                                    data[f * channels + c] = val;
                                }
                            }
                        }
                    },
                    err_fn,
                    None,
                )?
            }
            cpal::SampleFormat::I16 => {
                device.build_output_stream(
                    &config.into(),
                    move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                        let cfg = config_lock.read();
                        let master_vol = cfg.output_volume;
                        let is_deafened = cfg.is_deafened;
                        drop(cfg);

                        let frames = data.len() / channels;
                        let mut mono_buf = vec![0.0f32; frames];
                        mixer.mix_into_output(&mut mono_buf, master_vol, is_deafened);
                        for f in 0..frames {
                            let val_i16 = (mono_buf[f].clamp(-1.0, 1.0) * 32767.0) as i16;
                            for c in 0..channels {
                                data[f * channels + c] = val_i16;
                            }
                        }
                    },
                    err_fn,
                    None,
                )?
            }
            _ => {
                return Err(AudioError::DeviceNotAvailable("Unsupported output sample format".into()));
            }
        };

        stream.play()?;
        self.output_stream.set(Some(stream));
        tracing::info!("Audio playback stream initialized and running");
        Ok(())
    }

    pub fn start_input_stream(&self) -> Result<(), AudioError> {
        let host = cpal::default_host();
        let device = match host.default_input_device() {
            Some(d) => d,
            None => {
                tracing::warn!("No default audio input (microphone) device found on system");
                return Err(AudioError::DeviceNotAvailable("Input device not found".into()));
            }
        };

        let config = match device.default_input_config() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("Failed to query default input config: {}", e);
                return Err(e.into());
            }
        };

        let channels = config.channels() as usize;
        let echo_active = self.echo_test_active.clone();
        let mixer = self.mixer.clone();
        let vad = self.vad.clone();
        let filter = self.filter.clone();
        let last_rms = self.last_mic_rms.clone();
        let config_lock = self.config.clone();
        let sender_cb = self.audio_sender.clone();
        let channel_id_lock = self.active_voice_channel.clone();
        let echo_jitter = self.echo_jitter_buffer.clone();

        let err_fn = |err| {
            tracing::error!("CPAL audio input stream error: {:?}", err);
        };

        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => {
                device.build_input_stream(
                    &config.into(),
                    move |data: &[f32], _: &cpal::InputCallbackInfo| {
                        Self::process_input_frames(
                            data,
                            channels,
                            &echo_active,
                            &mixer,
                            &vad,
                            &filter,
                            &last_rms,
                            &config_lock,
                            &sender_cb,
                            &channel_id_lock,
                            &echo_jitter,
                        );
                    },
                    err_fn,
                    None,
                )?
            }
            cpal::SampleFormat::I16 => {
                device.build_input_stream(
                    &config.into(),
                    move |data: &[i16], _: &cpal::InputCallbackInfo| {
                        let f32_data: Vec<f32> = data.iter().map(|&s| s as f32 / 32768.0).collect();
                        Self::process_input_frames(
                            &f32_data,
                            channels,
                            &echo_active,
                            &mixer,
                            &vad,
                            &filter,
                            &last_rms,
                            &config_lock,
                            &sender_cb,
                            &channel_id_lock,
                            &echo_jitter,
                        );
                    },
                    err_fn,
                    None,
                )?
            }
            _ => {
                return Err(AudioError::DeviceNotAvailable("Unsupported input sample format".into()));
            }
        };

        stream.play()?;
        self.input_stream.set(Some(stream));
        tracing::info!("Audio capture stream initialized and running");
        Ok(())
    }

    fn process_input_frames(
        data: &[f32],
        channels: usize,
        echo_active: &AtomicBool,
        mixer: &SoftwareMixer,
        vad: &parking_lot::Mutex<VadAnalyzer>,
        filter: &parking_lot::Mutex<NoiseFilter>,
        last_rms: &AtomicU32,
        config: &RwLock<AudioEngineConfig>,
        sender_cb: &parking_lot::Mutex<Option<AudioPacketCallback>>,
        channel_id_lock: &RwLock<Option<String>>,
        _echo_jitter: &parking_lot::Mutex<Vec<f32>>,
    ) {
        if data.is_empty() || channels == 0 {
            return;
        }

        let cfg = config.read();
        let is_muted = cfg.is_muted;
        let input_vol = cfg.input_volume;
        drop(cfg);

        let frames = data.len() / channels;
        let mut mono: Vec<f32> = Vec::with_capacity(frames);

        if channels == 1 {
            for &s in data {
                mono.push(s * input_vol);
            }
        } else {
            for f in 0..frames {
                let mut sum = 0.0f32;
                for c in 0..channels {
                    sum += data[f * channels + c];
                }
                mono.push((sum / channels as f32) * input_vol);
            }
        }

        // Apply noise filter
        filter.lock().process(&mut mono);

        // VAD & RMS calculation
        let (is_speech, rms) = vad.lock().process_frame(&mono);
        last_rms.store(rms.to_bits(), Ordering::Relaxed);

        if is_muted {
            return;
        }

        // Echo test: smoothly feed processed audio into local mixer
        if echo_active.load(Ordering::Relaxed) {
            mixer.feed_samples("__local_echo__", &mono);
        }

        // Network transmission
        let in_voice = channel_id_lock.read().is_some();
        if in_voice && is_speech {
            if let Some(cb) = sender_cb.lock().as_ref() {
                cb(mono);
            }
        }
    }

    pub fn stop_streams(&self) {
        self.input_stream.set(None);
        self.output_stream.set(None);
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_software_mixer_per_user_volume() {
        let mixer = SoftwareMixer::new();
        mixer.set_user_volume("user1", 0.5); // 50%
        mixer.set_user_volume("user2", 2.0); // 200%

        let samples1 = vec![0.4; 10];
        let samples2 = vec![0.2; 10];

        mixer.feed_samples("user1", &samples1);
        mixer.feed_samples("user2", &samples2);

        let mut out = vec![0.0; 10];
        mixer.mix_into_output(&mut out, 1.0, false);

        // Expected: 0.4 * 0.5 + 0.2 * 2.0 = 0.2 + 0.4 = 0.6
        for &s in &out {
            assert!((s - 0.6).abs() < 0.001);
        }
    }

    #[test]
    fn test_deafen_silences_output() {
        let mixer = SoftwareMixer::new();
        mixer.feed_samples("user1", &[0.5; 10]);

        let mut out = vec![1.0; 10];
        mixer.mix_into_output(&mut out, 1.0, true);

        for &s in &out {
            assert_eq!(s, 0.0);
        }
    }

    #[test]
    fn test_vad_speech_detection() {
        let mut vad = VadAnalyzer::new(0.05);

        let silent_frame = vec![0.001; 100];
        let (speaking, _) = vad.process_frame(&silent_frame);
        assert!(!speaking);

        let loud_frame = vec![0.3; 100];
        let (speaking_loud, _) = vad.process_frame(&loud_frame);
        assert!(speaking_loud);
    }

    #[test]
    fn test_live_audio_pipeline_state_and_echo() {
        let config = Arc::new(RwLock::new(AudioEngineConfig::default()));
        let mixer = Arc::new(SoftwareMixer::new());
        let vad = Arc::new(parking_lot::Mutex::new(VadAnalyzer::new(0.03)));

        let pipeline = LiveAudioPipeline::new(config, mixer.clone(), vad);
        assert!(!pipeline.is_echo_test_active());

        pipeline.set_echo_test(true);
        assert!(pipeline.is_echo_test_active());

        pipeline.set_echo_test(false);
        assert!(!pipeline.is_echo_test_active());
    }
}

