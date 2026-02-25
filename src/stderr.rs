#[cfg(unix)]
mod unix {
    use std::io::{BufRead, BufReader};
    use std::os::unix::io::{AsRawFd, FromRawFd};
    use std::sync::mpsc;

    /// Redirect stderr (fd 2) to a pipe. Returns the receiver for captured lines
    /// and the saved fd to restore later.
    pub struct StderrCapture {
        saved_fd: i32,
        rx: mpsc::Receiver<String>,
    }

    impl StderrCapture {
        pub fn start() -> Option<Self> {
            let mut fds = [0i32; 2];
            if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
                return None;
            }
            let read_fd = fds[0];
            let write_fd = fds[1];

            let stderr_fd = std::io::stderr().as_raw_fd();
            let saved_fd = unsafe { libc::dup(stderr_fd) };
            if saved_fd < 0 {
                unsafe {
                    libc::close(read_fd);
                    libc::close(write_fd);
                }
                return None;
            }

            // Point stderr at the write end of the pipe
            unsafe { libc::dup2(write_fd, stderr_fd) };
            unsafe { libc::close(write_fd) };

            let (tx, rx) = mpsc::channel();

            // Background thread reads lines from the pipe
            std::thread::spawn(move || {
                let file = unsafe { std::fs::File::from_raw_fd(read_fd) };
                let reader = BufReader::new(file);
                for line in reader.lines() {
                    match line {
                        Ok(l) => {
                            if tx.send(l).is_err() {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
            });

            Some(Self { saved_fd, rx })
        }

        /// Drain any new lines into the provided vec.
        pub fn drain_into(&self, log: &mut Vec<String>) {
            while let Ok(line) = self.rx.try_recv() {
                log.push(line);
            }
        }

        /// Restore the original stderr fd.
        pub fn restore(self) {
            let stderr_fd = std::io::stderr().as_raw_fd();
            unsafe {
                libc::dup2(self.saved_fd, stderr_fd);
                libc::close(self.saved_fd);
            }
        }
    }
}

#[cfg(not(unix))]
mod non_unix {
    /// No-op stderr capture for non-Unix platforms (ALSA noise is Linux-specific).
    pub struct StderrCapture;

    impl StderrCapture {
        pub fn start() -> Option<Self> {
            None
        }

        pub fn drain_into(&self, _log: &mut Vec<String>) {}

        pub fn restore(self) {}
    }
}

#[cfg(unix)]
pub use unix::StderrCapture;
#[cfg(not(unix))]
pub use non_unix::StderrCapture;
