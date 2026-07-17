use std::io::Write;
use std::sync::{Arc, Mutex};

use tracing_subscriber::fmt::MakeWriter;

#[derive(Clone, Default)]
pub(crate) struct SharedLogWriter {
    bytes: Arc<Mutex<Vec<u8>>>,
}

pub(crate) struct SharedLogGuard {
    bytes: Arc<Mutex<Vec<u8>>>,
}

impl Write for SharedLogGuard {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.bytes
            .lock()
            .expect("log buffer mutex should not be poisoned")
            .extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'writer> MakeWriter<'writer> for SharedLogWriter {
    type Writer = SharedLogGuard;

    fn make_writer(&'writer self) -> Self::Writer {
        SharedLogGuard {
            bytes: Arc::clone(&self.bytes),
        }
    }
}

impl SharedLogWriter {
    pub(crate) fn contents(&self) -> String {
        String::from_utf8(
            self.bytes
                .lock()
                .expect("log buffer mutex should not be poisoned")
                .clone(),
        )
        .expect("structured log should be UTF-8")
    }
}
