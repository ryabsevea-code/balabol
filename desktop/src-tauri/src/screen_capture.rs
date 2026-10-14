use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use jpeg_encoder::{ColorType, Encoder};

#[cfg(windows)]
use windows_sys::Win32::Graphics::Gdi::{
    CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits,
    ReleaseDC, SelectObject, SetStretchBltMode, StretchBlt, BITMAPINFO, BITMAPINFOHEADER,
    BI_RGB, DIB_RGB_COLORS, HALFTONE, SRCCOPY,
};
#[cfg(windows)]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN,
};

pub struct ScreenCapturer {
    is_active: Arc<AtomicBool>,
}

impl ScreenCapturer {
    pub fn new() -> Self {
        Self {
            is_active: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn is_active(&self) -> bool {
        self.is_active.load(Ordering::Relaxed)
    }

    pub fn stop(&self) {
        self.is_active.store(false, Ordering::SeqCst);
    }

    pub fn start<F>(&self, on_frame: F)
    where
        F: Fn(Vec<u8>) + Send + Sync + 'static,
    {
        if self.is_active.swap(true, Ordering::SeqCst) {
            // Already running
            return;
        }

        let is_active = self.is_active.clone();

        std::thread::spawn(move || {
            let mut last_hash: u64 = 0;
            let mut unchanged_count: u32 = 0;

            // Target capture size: 960x540 (16:9, sharp text, ~15-20 KB JPEG)
            let target_w: u32 = 960;
            let target_h: u32 = 540;

            while is_active.load(Ordering::Relaxed) {
                let start_time = std::time::Instant::now();

                #[cfg(windows)]
                {
                    if let Some(rgb_data) = capture_screen_gdi(target_w, target_h) {
                        // Quick 64-bit sampling hash to detect static screen
                        let hash = compute_sampling_hash(&rgb_data);

                        let should_send = if hash == last_hash {
                            unchanged_count += 1;
                            // Send heartbeat once every 20 loops (~2.5 sec) even if static
                            if unchanged_count >= 20 {
                                unchanged_count = 0;
                                true
                            } else {
                                false
                            }
                        } else {
                            last_hash = hash;
                            unchanged_count = 0;
                            true
                        };

                        if should_send {
                            let mut jpeg_buf = Vec::new();
                            let encoder = Encoder::new(&mut jpeg_buf, 45); // quality 45 is crisp for text
                            if encoder
                                .encode(&rgb_data, target_w as u16, target_h as u16, ColorType::Rgb)
                                .is_ok()
                            {
                                on_frame(jpeg_buf);
                            }
                        }
                    }
                }

                // ~8 frames per second (125ms per frame)
                let elapsed = start_time.elapsed();
                let target_interval = Duration::from_millis(125);
                if elapsed < target_interval {
                    std::thread::sleep(target_interval - elapsed);
                }
            }
        });
    }
}

/// Compute a fast 64-bit sample hash of the RGB buffer to detect identical frames
fn compute_sampling_hash(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    // Sample every 64th byte to keep hashing under 50 microseconds
    let step = 64;
    let mut i = 0;
    while i < data.len() {
        h ^= data[i] as u64;
        h = h.wrapping_mul(0x100000001b3);
        i += step;
    }
    h
}

#[cfg(windows)]
fn capture_screen_gdi(target_w: u32, target_h: u32) -> Option<Vec<u8>> {
    unsafe {
        let hdc_screen = GetDC(std::ptr::null_mut());
        if hdc_screen.is_null() {
            return None;
        }

        let screen_w = GetSystemMetrics(SM_CXSCREEN);
        let screen_h = GetSystemMetrics(SM_CYSCREEN);
        if screen_w <= 0 || screen_h <= 0 {
            ReleaseDC(std::ptr::null_mut(), hdc_screen);
            return None;
        }

        let hdc_mem = CreateCompatibleDC(hdc_screen);
        if hdc_mem.is_null() {
            ReleaseDC(std::ptr::null_mut(), hdc_screen);
            return None;
        }

        let hbm = CreateCompatibleBitmap(hdc_screen, target_w as i32, target_h as i32);
        if hbm.is_null() {
            DeleteDC(hdc_mem);
            ReleaseDC(std::ptr::null_mut(), hdc_screen);
            return None;
        }

        let old_obj = SelectObject(hdc_mem, hbm);
        SetStretchBltMode(hdc_mem, HALFTONE);

        // Downscale desktop to target dimensions directly in GDI
        StretchBlt(
            hdc_mem,
            0,
            0,
            target_w as i32,
            target_h as i32,
            hdc_screen,
            0,
            0,
            screen_w,
            screen_h,
            SRCCOPY,
        );

        // Crucial Win32 rule: deselect hbm from hdc_mem BEFORE calling GetDIBits!
        SelectObject(hdc_mem, old_obj);

        let mut bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: target_w as i32,
                biHeight: -(target_h as i32), // Top-down DIB
                biPlanes: 1,
                biBitCount: 32, // 32-bit BGRA
                biCompression: BI_RGB,
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            bmiColors: [windows_sys::Win32::Graphics::Gdi::RGBQUAD {
                rgbBlue: 0,
                rgbGreen: 0,
                rgbRed: 0,
                rgbReserved: 0,
            }; 1],
        };

        let pixel_count = (target_w * target_h) as usize;
        let mut bgra_buf: Vec<u8> = vec![0u8; pixel_count * 4];

        let res = GetDIBits(
            hdc_screen,
            hbm,
            0,
            target_h,
            bgra_buf.as_mut_ptr() as _,
            &mut bi,
            DIB_RGB_COLORS,
        );

        // Clean up remaining GDI objects
        DeleteObject(hbm);
        DeleteDC(hdc_mem);
        ReleaseDC(std::ptr::null_mut(), hdc_screen);

        if res == 0 {
            return None;
        }

        // Convert BGRA to RGB (strip Alpha and swap B <-> R)
        let mut rgb_buf = Vec::with_capacity(pixel_count * 3);
        for chunk in bgra_buf.chunks_exact(4) {
            let b = chunk[0];
            let g = chunk[1];
            let r = chunk[2];
            rgb_buf.push(r);
            rgb_buf.push(g);
            rgb_buf.push(b);
        }

        Some(rgb_buf)
    }
}

#[cfg(not(windows))]
fn capture_screen_gdi(_target_w: u32, _target_h: u32) -> Option<Vec<u8>> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(windows)]
    fn test_capture_screen_gdi() {
        let res = capture_screen_gdi(960, 540);
        println!("test_capture_screen_gdi result is_some: {}", res.is_some());
        if let Some(ref data) = res {
            println!("test_capture_screen_gdi len: {}", data.len());
            let mut jpeg_buf = Vec::new();
            let encoder = Encoder::new(&mut jpeg_buf, 45);
            let enc_res = encoder.encode(data, 960, 540, ColorType::Rgb);
            println!("jpeg encode res: {:?}, jpeg len: {} bytes", enc_res.is_ok(), jpeg_buf.len());
            let chunk_count = (jpeg_buf.len() + 1199) / 1200;
            println!("chunk count: {}", chunk_count);
        }
        assert!(res.is_some(), "capture_screen_gdi failed and returned None!");
    }
}
