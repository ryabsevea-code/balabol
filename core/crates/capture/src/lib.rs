use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;
use xcap::{Monitor, Window};

#[derive(Error, Debug)]
pub enum CaptureError {
    #[error("XCap capture error: {0}")]
    XCap(#[from] xcap::XCapError),
    #[error("Target window or monitor not found: {0}")]
    TargetNotFound(String),
    #[error("Capture failed: empty frame")]
    EmptyFrame,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TargetKind {
    Monitor,
    Window,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureSource {
    pub id: String,
    pub title: String,
    pub kind: TargetKind,
    pub width: u32,
    pub height: u32,
    pub app_name: Option<String>,
}

#[derive(Clone)]
pub struct VideoRawFrame {
    pub width: u32,
    pub height: u32,
    pub rgba_data: Vec<u8>,
    pub timestamp_ms: u64,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Enumerate all available desktop monitors and application windows for screen sharing.
pub fn list_capture_sources() -> Result<Vec<CaptureSource>, CaptureError> {
    let mut sources = Vec::new();

    // 1. Monitors
    if let Ok(monitors) = Monitor::all() {
        for (i, m) in monitors.into_iter().enumerate() {
            let width = m.width();
            let height = m.height();
            let name = m.name().to_string();
            sources.push(CaptureSource {
                id: format!("mon_{}", i),
                title: name,
                kind: TargetKind::Monitor,
                width,
                height,
                app_name: None,
            });
        }
    }

    // 2. Windows
    if let Ok(windows) = Window::all() {
        for w in windows {
            let title = w.title();
            // Ignore minimized, empty, or hidden utility windows
            if title.trim().is_empty() || w.is_minimized() {
                continue;
            }

            let app_name = w.app_name().to_string();
            let width = w.width();
            let height = w.height();
            let id = format!("win_{}", w.id());

            sources.push(CaptureSource {
                id,
                title: title.to_string(),
                kind: TargetKind::Window,
                width,
                height,
                app_name: Some(app_name),
            });
        }
    }

    Ok(sources)
}

/// Capture a single frame from the specified monitor index.
pub fn capture_monitor_frame(monitor_index: usize) -> Result<VideoRawFrame, CaptureError> {
    let monitors = Monitor::all()?;
    let monitor = monitors
        .get(monitor_index)
        .ok_or_else(|| CaptureError::TargetNotFound(format!("Monitor index {}", monitor_index)))?;

    let image = monitor.capture_image()?;
    let width = image.width();
    let height = image.height();
    let rgba_data = image.into_raw();

    Ok(VideoRawFrame {
        width,
        height,
        rgba_data,
        timestamp_ms: now_ms(),
    })
}

/// Capture a single frame from a specific window by its numeric ID.
pub fn capture_window_frame(window_id: u32) -> Result<VideoRawFrame, CaptureError> {
    let windows = Window::all()?;
    let window = windows
        .into_iter()
        .find(|w| w.id() == window_id)
        .ok_or_else(|| CaptureError::TargetNotFound(format!("Window ID {}", window_id)))?;

    let image = window.capture_image()?;
    let width = image.width();
    let height = image.height();
    let rgba_data = image.into_raw();

    Ok(VideoRawFrame {
        width,
        height,
        rgba_data,
        timestamp_ms: now_ms(),
    })
}

/// Capture frame from either a monitor or window source ID (e.g. "mon_0" or "win_12345").
pub fn capture_source_frame(source_id: &str) -> Result<VideoRawFrame, CaptureError> {
    if let Some(rest) = source_id.strip_prefix("mon_") {
        let idx: usize = rest.parse().unwrap_or(0);
        capture_monitor_frame(idx)
    } else if let Some(rest) = source_id.strip_prefix("win_") {
        let wid: u32 = rest.parse().unwrap_or(0);
        capture_window_frame(wid)
    } else {
        capture_monitor_frame(0)
    }
}

/// Downscale if needed and compress raw RGBA frame to JPEG format for low-latency P2P transmission.
pub fn compress_frame_to_jpeg(frame: &VideoRawFrame, max_width: u32, quality: u8) -> Result<Vec<u8>, CaptureError> {
    let raw = match image::RgbaImage::from_raw(frame.width, frame.height, frame.rgba_data.clone()) {
        Some(img) => img,
        None => return Err(CaptureError::EmptyFrame),
    };

    let rgb_img = if frame.width > max_width {
        let target_height = ((frame.height as f32) * (max_width as f32 / frame.width as f32)) as u32;
        let resized = image::imageops::resize(&raw, max_width, target_height.max(1), image::imageops::FilterType::Triangle);
        image::DynamicImage::ImageRgba8(resized).to_rgb8()
    } else {
        image::DynamicImage::ImageRgba8(raw).to_rgb8()
    };

    let mut jpeg_bytes = Vec::new();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg_bytes, quality);
    encoder.encode_image(&rgb_img).map_err(|e| CaptureError::TargetNotFound(e.to_string()))?;

    Ok(jpeg_bytes)
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_sources_runs_without_panic() {
        // Enumerate sources should never crash the host
        let sources = list_capture_sources();
        assert!(sources.is_ok());
    }
}
