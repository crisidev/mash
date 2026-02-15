use tokio::fs::File;
use tokio::io::AsyncWriteExt;

pub(crate) struct Console {
    interactive: bool,
    last_status_length: usize,
    log_file: Option<File>,
}

impl Console {
    pub(crate) async fn new(interactive: bool, log_path: Option<String>) -> Self {
        let log_file = match log_path {
            Some(path) => tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .await
                .map_err(|e| eprintln!("Failed to open log file {}: {}", path, e))
                .ok(),
            None => None,
        };
        Self {
            interactive,
            last_status_length: 0,
            log_file,
        }
    }

    pub(crate) async fn output(&mut self, msg: &[u8]) {
        self.output_with_log(msg, None).await;
    }

    pub(crate) async fn output_with_log(&mut self, msg: &[u8], log_msg: Option<&[u8]>) {
        self.log(log_msg.unwrap_or(msg)).await;
        if self.interactive && self.last_status_length > 0 {
            let clear = format!("\r{}\r", " ".repeat(self.last_status_length));
            safe_write(clear.as_bytes()).await;
            self.last_status_length = 0;
        }
        safe_write(msg).await;
    }

    pub(crate) async fn log(&mut self, msg: &[u8]) {
        if let Some(ref mut f) = self.log_file {
            let _ = f.write_all(msg).await;
        }
    }

    pub(crate) fn set_last_status_length(&mut self, length: usize) {
        self.last_status_length = length;
    }

    pub(crate) async fn set_log_file(&mut self, path: Option<&str>) {
        self.log_file = match path {
            Some(p) => tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)
                .await
                .map_err(|e| eprintln!("Failed to open log file {}: {}", p, e))
                .ok(),
            None => None,
        };
    }

    pub(crate) fn disable_log(&mut self) {
        self.log_file = None;
    }

    pub(crate) fn has_log(&self) -> bool {
        self.log_file.is_some()
    }
}

async fn safe_write(buf: &[u8]) {
    let mut stdout = tokio::io::stdout();
    let _ = stdout.write_all(buf).await;
    let _ = stdout.flush().await;
}
