//! Updating kimo without stopping its agents. A restart execs the new binary in the same
//! process, which keeps its child processes and the files it does not close: before the exec,
//! kimo writes down each running terminal (its PTY's file descriptor, the agent's process,
//! the terminal's size and recent screen) and keeps those descriptors open; the new kimo
//! adopts them. Claude Code and Codex never notice. A terminal that cannot be adopted falls
//! back to the restart-and-continue of before.

use super::*;
use std::os::fd::{FromRawFd, OwnedFd, RawFd};

/// A terminal handed to the next kimo.
#[derive(serde::Serialize, serde::Deserialize)]
struct Handed {
    id: String,
    fd: RawFd,
    pid: i32,
    rows: u16,
    cols: u16,
    paused: bool,
}

fn set_cloexec(fd: RawFd, on: bool) -> std::io::Result<()> {
    // SAFETY: fcntl on a descriptor this process owns; only the close-on-exec flag changes.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFD);
        if flags < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let flags = if on {
            flags | libc::FD_CLOEXEC
        } else {
            flags & !libc::FD_CLOEXEC
        };
        if libc::fcntl(fd, libc::F_SETFD, flags) < 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

/// The agent's process, adopted from the kimo before: still this process's child after the
/// exec, so it is waited for and signalled by pid.
#[derive(Debug)]
struct AdoptedChild {
    pid: i32,
}

fn exit_status(raw: libc::c_int) -> portable_pty::ExitStatus {
    if libc::WIFEXITED(raw) {
        portable_pty::ExitStatus::with_exit_code(libc::WEXITSTATUS(raw) as u32)
    } else {
        portable_pty::ExitStatus::with_signal("signal")
    }
}

impl portable_pty::ChildKiller for AdoptedChild {
    fn kill(&mut self) -> std::io::Result<()> {
        // SAFETY: a signal to the adopted agent.
        if unsafe { libc::kill(self.pid, libc::SIGKILL) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }

    fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
        Box::new(AdoptedChild { pid: self.pid })
    }
}

impl Child for AdoptedChild {
    fn try_wait(&mut self) -> std::io::Result<Option<portable_pty::ExitStatus>> {
        let mut raw = 0;
        // SAFETY: waitpid on our own child, without blocking.
        match unsafe { libc::waitpid(self.pid, &mut raw, libc::WNOHANG) } {
            0 => Ok(None),
            p if p == self.pid => Ok(Some(exit_status(raw))),
            // Already gone and reaped (ECHILD): ended.
            _ => Ok(Some(portable_pty::ExitStatus::with_exit_code(1))),
        }
    }

    fn wait(&mut self) -> std::io::Result<portable_pty::ExitStatus> {
        let mut raw = 0;
        // SAFETY: waitpid on our own child.
        match unsafe { libc::waitpid(self.pid, &mut raw, 0) } {
            p if p == self.pid => Ok(exit_status(raw)),
            _ => Ok(portable_pty::ExitStatus::with_exit_code(1)),
        }
    }

    fn process_id(&self) -> Option<u32> {
        Some(self.pid as u32)
    }
}

/// The PTY's master side, adopted by its file descriptor.
struct AdoptedMaster {
    fd: OwnedFd,
}

impl AdoptedMaster {
    fn dup(&self) -> Result<std::fs::File, anyhow::Error> {
        Ok(std::fs::File::from(self.fd.try_clone()?))
    }
}

impl MasterPty for AdoptedMaster {
    fn resize(&self, size: PtySize) -> Result<(), anyhow::Error> {
        let ws = libc::winsize {
            ws_row: size.rows,
            ws_col: size.cols,
            ws_xpixel: size.pixel_width,
            ws_ypixel: size.pixel_height,
        };
        // SAFETY: TIOCSWINSZ with a valid winsize on our PTY master.
        if unsafe { libc::ioctl(self.fd.as_raw_fd_compat(), libc::TIOCSWINSZ, &ws) } < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }

    fn get_size(&self) -> Result<PtySize, anyhow::Error> {
        let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
        // SAFETY: TIOCGWINSZ fills `ws`.
        if unsafe { libc::ioctl(self.fd.as_raw_fd_compat(), libc::TIOCGWINSZ, &mut ws) } < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(PtySize {
            rows: ws.ws_row,
            cols: ws.ws_col,
            pixel_width: ws.ws_xpixel,
            pixel_height: ws.ws_ypixel,
        })
    }

    fn try_clone_reader(&self) -> Result<Box<dyn std::io::Read + Send>, anyhow::Error> {
        Ok(Box::new(self.dup()?))
    }

    fn take_writer(&self) -> Result<Box<dyn std::io::Write + Send>, anyhow::Error> {
        Ok(Box::new(self.dup()?))
    }

    fn process_group_leader(&self) -> Option<libc::pid_t> {
        // SAFETY: tcgetpgrp on our PTY master.
        match unsafe { libc::tcgetpgrp(self.fd.as_raw_fd_compat()) } {
            p if p > 0 => Some(p),
            _ => None,
        }
    }

    fn as_raw_fd(&self) -> Option<RawFd> {
        Some(self.fd.as_raw_fd_compat())
    }

    fn tty_name(&self) -> Option<std::path::PathBuf> {
        None
    }
}

/// `AsRawFd` under another name: `MasterPty` has an `as_raw_fd` of its own.
trait RawFdCompat {
    fn as_raw_fd_compat(&self) -> RawFd;
}

impl RawFdCompat for OwnedFd {
    fn as_raw_fd_compat(&self) -> RawFd {
        std::os::fd::AsRawFd::as_raw_fd(self)
    }
}

impl Manager {
    fn handoff_dir(&self) -> PathBuf {
        self.cfg.data_dir.join("handoff")
    }

    /// Before exec'ing the new kimo: hand every running terminal over (they keep running).
    /// Returns how many. On an error nothing is handed over: stop them as before.
    pub fn hand_over(&self) -> Result<usize> {
        let dir = self.handoff_dir();
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        let mut inner = self.lock();
        let mut handed = Vec::new();
        for (id, live) in &inner.live {
            let Some(rt) = &live.runtime else { continue };
            let Some(fd) = rt.master.as_raw_fd() else {
                continue;
            };
            std::fs::write(
                dir.join(format!("{id}.screen")),
                live.scrollback.lock().unwrap().snapshot(),
            )?;
            handed.push(Handed {
                id: id.clone(),
                fd,
                pid: rt.pid,
                rows: live.size.rows,
                cols: live.size.cols,
                paused: live.status == SessionStatus::Paused,
            });
        }
        std::fs::write(dir.join("sessions.json"), serde_json::to_vec(&handed)?)?;
        // Only now (nothing can fail any more): the descriptors stay open across the exec,
        // and the runtimes are not dropped (that would close them).
        for h in &handed {
            set_cloexec(h.fd, false)?;
        }
        for live in inner.live.values_mut() {
            if let Some(rt) = live.runtime.take() {
                std::mem::forget(rt);
            }
        }
        Ok(handed.len())
    }

    /// At start, after an update: adopt the terminals the kimo before handed over. Those that
    /// cannot be adopted are ended; the restart's resume flag starts them again.
    pub fn adopt_handed_over(&self) -> usize {
        let dir = self.handoff_dir();
        let Ok(text) = std::fs::read(dir.join("sessions.json")) else {
            return 0;
        };
        let handed: Vec<Handed> = serde_json::from_slice(&text).unwrap_or_default();
        let mut adopted = 0;
        for h in handed {
            match self.adopt(&dir, &h) {
                Ok(()) => adopted += 1,
                Err(e) => {
                    tracing::warn!("cannot keep {} running across the update: {e:#}", h.id);
                    // Its old process must not linger: it starts again (continuing) instead.
                    signal_group(h.pid, libc::SIGHUP);
                    // SAFETY: a descriptor inherited for this terminal and not adopted.
                    unsafe {
                        libc::close(h.fd);
                    }
                }
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
        adopted
    }

    fn adopt(&self, dir: &std::path::Path, h: &Handed) -> Result<()> {
        // The descriptor is open and a terminal, and the agent is still our running child.
        // SAFETY: plain checks on an inherited descriptor and on a pid.
        unsafe {
            if libc::fcntl(h.fd, libc::F_GETFD) < 0 || libc::isatty(h.fd) != 1 {
                bail!("its terminal did not come across");
            }
            let mut raw = 0;
            if libc::waitpid(h.pid, &mut raw, libc::WNOHANG) != 0 {
                bail!("its agent is not running any more");
            }
            // Blocking reads, as for a new terminal.
            let fl = libc::fcntl(h.fd, libc::F_GETFL);
            if fl >= 0 {
                libc::fcntl(h.fd, libc::F_SETFL, fl & !libc::O_NONBLOCK);
            }
        }
        // Agents started from now on must not inherit it.
        set_cloexec(h.fd, true)?;
        let mut inner = self.lock();
        if !inner.state.sessions.iter().any(|s| s.id == h.id) {
            bail!("the session is gone");
        }
        let cap = self.cfg.scrollback_bytes;
        let live = inner
            .live
            .entry(h.id.clone())
            .or_insert_with(|| new_live(cap));
        if live.runtime.is_some() {
            bail!("already running");
        }
        // SAFETY: the descriptor was handed over for this terminal and is owned from here on.
        let master = AdoptedMaster {
            fd: unsafe { OwnedFd::from_raw_fd(h.fd) },
        };
        live.size = PtySize {
            rows: h.rows,
            cols: h.cols,
            pixel_width: 0,
            pixel_height: 0,
        };
        live.generation += 1;
        if let Ok(screen) = std::fs::read(dir.join(format!("{}.screen", h.id))) {
            live.scrollback.lock().unwrap().push(&screen);
        }
        let note = b"\r\n\x1b[2m[kimo] updated: still running\x1b[0m\r\n";
        live.scrollback.lock().unwrap().push(note);
        let runtime = self.wire(
            &h.id,
            live,
            Box::new(master),
            Box::new(AdoptedChild { pid: h.pid }),
            h.pid,
        )?;
        live.runtime = Some(runtime);
        live.status = if h.paused {
            SessionStatus::Paused
        } else {
            SessionStatus::Running
        };
        live.exit_code = None;
        live.last_ticks = None;
        if let Some(spec) = inner.state.sessions.iter_mut().find(|s| s.id == h.id) {
            // It never stopped: nothing to start again.
            spec.resume_on_start = false;
        }
        self.save(&inner)
    }
}
