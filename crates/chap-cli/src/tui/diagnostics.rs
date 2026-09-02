use chap_core::SessionId;
use std::{
    backtrace::Backtrace,
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    panic::{self, PanicHookInfo},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};

type PanicHook = Box<dyn Fn(&PanicHookInfo<'_>) + Send + Sync + 'static>;

pub(super) struct TuiDiagnostics {
    path: PathBuf,
    writer: DiagnosticWriter,
    previous_hook: Option<PanicHook>,
}

impl TuiDiagnostics {
    pub(super) fn install(consent_path: &Path, session_id: SessionId) -> Result<Self, String> {
        let path = consent_path.with_file_name(format!("tui-{session_id}.log"));
        let parent = path
            .parent()
            .ok_or_else(|| format!("diagnostics path has no parent: {}", path.display()))?;
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "cannot create diagnostics directory {}: {error}",
                parent.display()
            )
        })?;
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .map_err(|error| {
                format!("cannot create diagnostics log {}: {error}", path.display())
            })?;
        let writer = DiagnosticWriter::new(file);
        let hook_writer = writer.clone();
        let previous_hook = panic::take_hook();
        panic::set_hook(Box::new(move |info| hook_writer.write_panic(info)));

        Ok(Self {
            path,
            writer,
            previous_hook: Some(previous_hook),
        })
    }

    pub(super) fn writer(&self) -> DiagnosticWriter {
        self.writer.clone()
    }

    pub(super) fn finish(mut self) {
        self.restore_hook();
        let _ = self.writer.flush();
        let _ = write_notice_if_nonempty(&self.path, &mut io::stderr().lock());
    }

    fn restore_hook(&mut self) {
        if std::thread::panicking() {
            return;
        }
        if let Some(hook) = self.previous_hook.take() {
            panic::set_hook(hook);
        }
    }
}

impl Drop for TuiDiagnostics {
    fn drop(&mut self) {
        self.restore_hook();
    }
}

#[derive(Clone)]
pub(super) struct DiagnosticWriter {
    sink: Arc<Mutex<Box<dyn Write + Send>>>,
}

impl DiagnosticWriter {
    fn new(sink: impl Write + Send + 'static) -> Self {
        Self {
            sink: Arc::new(Mutex::new(Box::new(sink))),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Box<dyn Write + Send>> {
        self.sink
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn write_panic(&self, info: &PanicHookInfo<'_>) {
        let thread = std::thread::current();
        let thread_name = thread.name().unwrap_or("unnamed");
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| info.payload().downcast_ref::<String>().map(String::as_str))
            .unwrap_or("Box<dyn Any>");
        let backtrace = Backtrace::force_capture();
        self.write_panic_diagnostic(thread_name, message, info.location(), &backtrace);
    }

    fn write_panic_diagnostic(
        &self,
        thread_name: &str,
        message: &str,
        location: Option<&panic::Location<'_>>,
        backtrace: &dyn fmt::Display,
    ) {
        let _ = format_panic_diagnostic(
            &mut **self.lock(),
            thread_name,
            message,
            location,
            backtrace,
        );
    }
}

impl Write for DiagnosticWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.lock().write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.lock().flush()
    }
}

fn format_panic_diagnostic(
    sink: &mut dyn Write,
    thread_name: &str,
    message: &str,
    location: Option<&panic::Location<'_>>,
    backtrace: &dyn fmt::Display,
) -> io::Result<()> {
    write!(sink, "thread '{thread_name}' panicked")?;
    if let Some(location) = location {
        write!(sink, " at {location}")?;
    }
    writeln!(sink, ":\n{message}\nstack backtrace:\n{backtrace}")
}

fn write_notice_if_nonempty(path: &Path, notice: &mut dyn Write) -> io::Result<bool> {
    if File::open(path)?.metadata()?.len() == 0 {
        let _ = fs::remove_file(path);
        return Ok(false);
    }

    writeln!(notice, "Diagnostics written to {}", path.display())?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panic_diagnostic_uses_the_injected_log_sink_not_stderr() {
        let directory = tempfile::tempdir().unwrap();
        let log_path = directory.path().join("session.log");
        let mut writer = DiagnosticWriter::new(File::create(&log_path).unwrap());
        let stderr = Vec::<u8>::new();

        writer.write_panic_diagnostic("worker", "boom", None, &"test backtrace");
        writer.flush().unwrap();

        let log = fs::read_to_string(log_path).unwrap();
        assert!(log.contains("thread 'worker' panicked:\nboom"));
        assert!(log.contains("stack backtrace:\ntest backtrace"));
        assert!(stderr.is_empty());
    }

    #[test]
    fn log_notice_is_printed_only_for_a_nonempty_file() {
        let directory = tempfile::tempdir().unwrap();
        let empty_path = directory.path().join("empty.log");
        fs::write(&empty_path, []).unwrap();
        let mut notice = Vec::new();

        assert!(!write_notice_if_nonempty(&empty_path, &mut notice).unwrap());
        assert!(notice.is_empty());

        let log_path = directory.path().join("session.log");
        fs::write(&log_path, "diagnostic").unwrap();
        assert!(write_notice_if_nonempty(&log_path, &mut notice).unwrap());
        assert_eq!(
            String::from_utf8(notice).unwrap(),
            format!("Diagnostics written to {}\n", log_path.display())
        );
    }
}
