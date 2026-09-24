use parking_lot::RwLock;
use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone)]
pub struct AppLogger {
    file_path: PathBuf,
    recent_logs: Arc<RwLock<VecDeque<String>>>,
}

impl AppLogger {
    pub fn init() -> Self {
        let app_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let file_path = app_dir.join("balabol.log");

        // Write session start marker
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&file_path) {
            let _ = writeln!(
                f,
                "\n==================== BALABOL SESSION STARTED [{}] ====================",
                format_timestamp()
            );
            let _ = f.flush();
        }

        Self {
            file_path,
            recent_logs: Arc::new(RwLock::new(VecDeque::with_capacity(200))),
        }
    }

    pub fn log(&self, level: &str, message: &str) {
        let timestamp = format_timestamp();
        let formatted = format!("[{}] [{}] {}", timestamp, level, message);

        // 1. Output to stdout/terminal for dev mode
        println!("{}", formatted);

        // 2. Append to external file balabol.log with immediate flush
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.file_path)
        {
            let _ = writeln!(file, "{}", formatted);
            let _ = file.flush();
        }

        // 3. Keep in recent memory buffer for UI live display
        let mut buffer = self.recent_logs.write();
        if buffer.len() >= 200 {
            buffer.pop_front();
        }
        buffer.push_back(formatted);
    }

    pub fn info(&self, message: &str) {
        self.log("INFO", message);
    }

    pub fn warn(&self, message: &str) {
        self.log("WARN", message);
    }

    pub fn error(&self, message: &str) {
        self.log("ERROR", message);
    }

    pub fn get_recent_logs(&self) -> Vec<String> {
        self.recent_logs.read().iter().cloned().collect()
    }

    pub fn log_file_path(&self) -> String {
        self.file_path.to_string_lossy().to_string()
    }
}

fn format_timestamp() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let total_secs = now.as_secs();
    let millis = now.subsec_millis();

    let hours = (total_secs / 3600) % 24;
    let minutes = (total_secs / 60) % 60;
    let seconds = total_secs % 60;

    format!("{:02}:{:02}:{:02}.{:03}", hours, minutes, seconds, millis)
}
