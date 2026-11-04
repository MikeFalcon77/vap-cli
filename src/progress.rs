use std::io::{self, IsTerminal, Write};

pub struct Progress {
    task: Option<tokio::task::JoinHandle<()>>,
}
impl Progress {
    pub fn new(message: String, json: bool) -> Self {
        let enabled = !json
            && io::stderr().is_terminal()
            && std::env::var("TERM").as_deref() != Ok("dumb")
            && std::env::var_os("NO_COLOR").is_none();
        let task = enabled.then(|| {
            tokio::spawn(async move {
                let frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
                let started = std::time::Instant::now();
                let mut i = 0;
                loop {
                    eprint!(
                        "\r\x1b[2K\x1b[36m{}\x1b[0m {} · {}s",
                        frames[i % frames.len()],
                        message,
                        started.elapsed().as_secs()
                    );
                    let _ = io::stderr().flush();
                    i += 1;
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            })
        });
        Self { task }
    }
}
impl Drop for Progress {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
            eprint!("\r\x1b[2K");
            let _ = io::stderr().flush();
        }
    }
}
