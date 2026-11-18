use super::{schedule::Trigger, signals, state::atomic_json};
use crate::native::{env as bob_env, spinner::Spinner};
use chrono::{DateTime, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    io::{self, IsTerminal},
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

#[derive(Serialize, Deserialize)]
struct Running {
    pid: u32,
    started_at: DateTime<Utc>,
    trigger: Trigger,
    current_step: Option<String>,
}
pub(super) struct RunLock {
    _file: File,
    dir: PathBuf,
    running: Running,
}
impl RunLock {
    pub fn acquire(
        dir: &Path,
        started_at: DateTime<Utc>,
        trigger: Trigger,
        quiet: bool,
    ) -> io::Result<Result<Self, String>> {
        let file = bob_env::create_state_lock_file(dir, "run.lock")?;
        let interactive =
            io::stdin().is_terminal() && io::stdout().is_terminal();
        let mut spinner = None;
        let mut label = String::new();
        loop {
            if signals::number() != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "interrupted while waiting for tick",
                ));
            }
            match file.try_lock_exclusive() {
                Ok(()) => break,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    let busy = busy_message(dir);
                    if !interactive {
                        return Ok(Err(busy));
                    }
                    if !quiet && busy != label {
                        drop(spinner.take());
                        spinner = Some(Spinner::start(&busy));
                        label = busy;
                    }
                    thread::sleep(Duration::from_millis(100));
                }
                Err(e) => return Err(e),
            }
        }
        drop(spinner);
        let lock = Self {
            _file: file,
            dir: dir.into(),
            running: Running {
                pid: std::process::id(),
                started_at,
                trigger,
                current_step: None,
            },
        };
        atomic_json(dir, "running.json", &lock.running)?;
        Ok(Ok(lock))
    }
    pub fn step(&mut self, name: &str) -> io::Result<()> {
        self.running.current_step = Some(name.into());
        atomic_json(&self.dir, "running.json", &self.running)
    }
}
impl Drop for RunLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(self.dir.join("running.json"));
    }
}
fn busy_message(dir: &Path) -> String {
    let running = fs::read(dir.join("running.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Running>(&b).ok());
    match running {
        Some(r) => format!(
            "already running · started {} · on {}",
            super::render::clock(r.started_at),
            r.current_step.as_deref().unwrap_or("starting")
        ),
        None => "already running".into(),
    }
}
