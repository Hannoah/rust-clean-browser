//! Single instance: launching the browser with a file/URL (e.g. double-clicking an .html file)
//! while it is already running opens a tab in the existing window instead of a new window.
//!
//! The running instance listens on a loopback port and records `<port> <token>` in
//! `<app data>/instance`. A second launch reads it, sends `token\nurl\n` and waits for `ok`
//! (so a stale port reused by some other program can't swallow the URL). The random token
//! keeps web pages and other local users from driving the browser through that port.

use crate::bookmarks::app_data_dir;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

static INSTANCE_OWNER: OnceLock<(PathBuf, String)> = OnceLock::new();

/// Remove `<app data>/instance` on exit if this process created it.
pub fn cleanup() {
    if let Some((path, token)) = INSTANCE_OWNER.get()
        && std::fs::read_to_string(path)
            .ok()
            .is_some_and(|info| info.trim().ends_with(token.as_str()))
    {
        let _ = std::fs::remove_file(path);
    }
}

/// 128 random bits as hex, straight from the OS CSPRNG.
pub fn random_token() -> String {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).expect("OS RNG unavailable");
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn info_path() -> Option<PathBuf> {
    app_data_dir().map(|d| d.join("instance"))
}

/// Hand `arg` (path or URL) to a running instance. `true` = it opened a tab; caller should exit.
pub fn forward(arg: &str) -> bool {
    info_path().and_then(|p| try_forward_at(&p, arg)).is_some()
}

fn try_forward_at(path: &std::path::Path, arg: &str) -> Option<()> {
    let info = std::fs::read_to_string(path).ok()?;
    let (port, token) = info.trim().split_once(' ')?;
    // This process's working directory differs from the running one: send absolute paths.
    let arg = std::path::absolute(arg)
        .ok()
        .filter(|p| p.is_file())
        .map_or(arg.to_string(), |p| p.display().to_string());
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port.parse().ok()?));
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_millis(500)).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(3))).ok()?;
    s.set_write_timeout(Some(Duration::from_secs(2))).ok()?;
    s.write_all(format!("{token}\n{}\n", arg.replace(['\r', '\n'], "")).as_bytes())
        .ok()?;
    let mut ack = [0u8; 2];
    s.read_exact(&mut ack).ok()?;
    (&ack == b"ok").then_some(())
}

/// Start accepting forwarded URLs; `on_url` runs on the listener thread.
pub fn listen(on_url: impl Fn(String) + Send + Sync + 'static) {
    let Some(path) = info_path() else { return };
    listen_at(&path, Arc::new(on_url));
}

fn listen_at(path: &std::path::Path, on_url: Arc<dyn Fn(String) + Send + Sync>) {
    let Ok(listener) = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)) else {
        return;
    };
    let Ok(addr) = listener.local_addr() else { return };
    let token = random_token();
    let payload = format!("{} {token}", addr.port());
    if crate::net::atomic_write_private(path, payload.as_bytes()).is_err() {
        return;
    }
    let _ = INSTANCE_OWNER.set((path.to_path_buf(), token.clone()));
    let active = Arc::new(AtomicUsize::new(0));
    std::thread::spawn(move || {
        for s in listener.incoming().flatten() {
            if active.fetch_add(1, Ordering::AcqRel) >= 8 {
                active.fetch_sub(1, Ordering::AcqRel);
                continue;
            }
            let active_c = Arc::clone(&active);
            let token_c = token.clone();
            let cb = on_url.clone();
            std::thread::spawn(move || {
                struct Guard(Arc<AtomicUsize>);
                impl Drop for Guard {
                    fn drop(&mut self) {
                        self.0.fetch_sub(1, Ordering::AcqRel);
                    }
                }
                let _guard = Guard(active_c);
                let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
                let _ = s.set_write_timeout(Some(Duration::from_secs(2)));
                let mut lines = BufReader::new((&s).take(8192)).lines();
                if lines.next().and_then(Result::ok).as_deref() != Some(token_c.as_str()) {
                    return;
                }
                if let Some(Ok(url)) = lines.next()
                    && (&s).write_all(b"ok").is_ok()
                {
                    cb(url);
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_wrong_token_and_delivers_right_one() {
        let (tx, rx) = std::sync::mpsc::channel();
        let dir = std::env::temp_dir().join(format!("rcb-single-{}-{}", std::process::id(), random_token()));
        let path = dir.join("instance");
        listen_at(&path, Arc::new(move |u| tx.send(u).unwrap()));
        assert!(try_forward_at(&path, "https://example.com/a").is_some());
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            "https://example.com/a"
        );

        let info = std::fs::read_to_string(&path).unwrap();
        let (p, _) = info.split_once(' ').unwrap();
        let mut bad = TcpStream::connect(("127.0.0.1", p.parse::<u16>().unwrap())).unwrap();
        bad.write_all(b"wrong\nhttps://evil.test\n").unwrap();
        assert!(rx.recv_timeout(Duration::from_millis(500)).is_err());

        // Valid token followed by invalid UTF-8 URL must not return "ok".
        let (_, tok) = info.trim().split_once(' ').unwrap();
        let mut invalid = TcpStream::connect(("127.0.0.1", p.parse::<u16>().unwrap())).unwrap();
        invalid.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
        invalid.write_all(format!("{tok}\n").as_bytes()).unwrap();
        invalid.write_all(&[0xff, 0xfe, b'\n']).unwrap();
        let mut ack = [0u8; 2];
        assert!(invalid.read_exact(&mut ack).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
