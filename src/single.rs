//! Single instance: launching the browser with a file/URL (e.g. double-clicking an .html file)
//! while it is already running opens a tab in the existing window instead of a new window.
//!
//! The running instance listens on a loopback port and records `<port> <token>` in
//! `<app data>/instance`. A second launch reads it, sends `token\nurl\n` and waits for `ok`
//! (so a stale port reused by some other program can't swallow the URL). The random token
//! keeps web pages and other local users from driving the browser through that port.

use crate::bookmarks::app_data_dir;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::time::Duration;

/// 128 random bits as hex. `RandomState` is seeded from the OS RNG: unpredictable without a dependency.
pub fn random_token() -> String {
    let rand = || RandomState::new().build_hasher().finish();
    format!("{:016x}{:016x}", rand(), rand())
}

fn info_path() -> Option<PathBuf> {
    app_data_dir().map(|d| d.join("instance"))
}

/// Hand `arg` (path or URL) to a running instance. `true` = it opened a tab; caller should exit.
pub fn forward(arg: &str) -> bool {
    try_forward(arg).is_some()
}

fn try_forward(arg: &str) -> Option<()> {
    let info = std::fs::read_to_string(info_path()?).ok()?;
    let (port, token) = info.trim().split_once(' ')?;
    // This process's working directory differs from the running one: send absolute paths.
    let arg = std::path::absolute(arg).ok().filter(|p| p.is_file()).map_or(arg.to_string(), |p| p.display().to_string());
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port.parse().ok()?));
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_millis(500)).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(3))).ok()?;
    s.write_all(format!("{token}\n{}\n", arg.replace(['\r', '\n'], "")).as_bytes()).ok()?;
    let mut ack = [0u8; 2];
    s.read_exact(&mut ack).ok()?;
    (&ack == b"ok").then_some(())
}

/// Start accepting forwarded URLs; `on_url` runs on the listener thread.
pub fn listen(on_url: impl Fn(String) + Send + 'static) {
    let Some(path) = info_path() else { return };
    let Ok(listener) = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)) else { return };
    let Ok(addr) = listener.local_addr() else { return };
    let token = random_token();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if std::fs::write(&path, format!("{} {token}", addr.port())).is_err() {
        return;
    }
    std::thread::spawn(move || {
        for mut s in listener.incoming().flatten() {
            let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
            let mut lines = BufReader::new((&s).take(8192)).lines();
            if lines.next().and_then(Result::ok).as_deref() != Some(token.as_str()) {
                continue;
            }
            if let Some(Ok(url)) = lines.next().filter(|_| s.write_all(b"ok").is_ok()) {
                on_url(url);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_wrong_token_and_delivers_right_one() {
        let (tx, rx) = std::sync::mpsc::channel();
        let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        // Exercise the wire protocol through the real listener via a temp data dir.
        // SAFETY: test-only env mutation, no other test reads LOCALAPPDATA/HOME concurrently.
        let dir = std::env::temp_dir().join(format!("rcb-single-{port}"));
        unsafe {
            std::env::set_var("LOCALAPPDATA", &dir);
            std::env::set_var("HOME", &dir);
            std::env::set_var("XDG_DATA_HOME", &dir);
        }
        listen(move |u| tx.send(u).unwrap());
        assert!(forward("https://example.com/a"));
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), "https://example.com/a");

        let info = std::fs::read_to_string(info_path().unwrap()).unwrap();
        let (p, _) = info.split_once(' ').unwrap();
        let mut bad = TcpStream::connect(("127.0.0.1", p.parse::<u16>().unwrap())).unwrap();
        bad.write_all(b"wrong\nhttps://evil.test\n").unwrap();
        assert!(rx.recv_timeout(Duration::from_millis(500)).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
