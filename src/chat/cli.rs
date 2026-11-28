#![cfg_attr(not(windows), allow(clippy::drop_non_drop))]

use crate::bookmarks::app_data_dir;
use serde_json::{Value, json};
use std::io::{BufRead as _, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::prompt::{build_claude_turn_prompt, build_cli_prompt, claude_system_prompt};
use super::{
    AgentContext, CLI_TIMEOUT, ChatMessage, ChatProvider, MAX_CLAUDE_WORKER_TURNS, ModelOption, PageContext,
    WORKER_IDLE_TIMEOUT, is_valid_effort, is_valid_model_id,
};

pub fn parse_agy_models_output(raw: &str) -> Vec<ModelOption> {
    let mut out = Vec::new();
    for line in raw.lines() {
        let clean = strip_ansi(line);
        let trimmed = clean.trim();
        if let Some((id, label)) = trimmed.split_once('\t') {
            let id = id.trim();
            let label = label.trim();
            if is_valid_model_id(id) && !label.is_empty() && !out.iter().any(|m: &ModelOption| m.id == id) {
                out.push(ModelOption {
                    id: id.to_string(),
                    label: label.to_string(),
                });
            }
        }
    }
    out
}

pub async fn fetch_agy_models() -> Vec<ModelOption> {
    tokio::task::spawn_blocking(fetch_agy_models_blocking)
        .await
        .unwrap_or_default()
}

pub(super) fn fetch_agy_models_blocking() -> Vec<ModelOption> {
    let Some(bin_path) = find_cli_binary("agy") else {
        return Vec::new();
    };
    let Ok(mut cmd) = build_cli_command(&bin_path, &["models".to_string()]) else {
        return Vec::new();
    };
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let Ok(mut child) = cmd.spawn() else {
        return Vec::new();
    };
    let job = win_job::ProcessJob::attach(&child);
    let out_pipe = child.stdout.take();
    let err_pipe = child.stderr.take();
    let out_reader = std::thread::spawn(move || read_pipe_to_string(out_pipe));
    let err_reader = std::thread::spawn(move || read_pipe_to_string(err_pipe));

    let started = Instant::now();
    let mut timed_out = false;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < Duration::from_secs(15) => {
                std::thread::sleep(Duration::from_millis(50));
            }
            _ => {
                timed_out = true;
                if let Some(j) = &job {
                    j.terminate();
                }
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
        }
    }
    drop(job);
    if timed_out {
        return Vec::new();
    }
    let stdout = out_reader.join().unwrap_or_default();
    let _ = err_reader.join();
    parse_agy_models_output(&stdout)
}

/// Empty working directory for a CLI agent, so it never starts inside the user's files.
pub(super) fn empty_cwd(name: &str) -> PathBuf {
    let dir = app_data_dir()
        .map(|p| p.join(name))
        .unwrap_or_else(|| std::env::temp_dir().join(format!("rb-{name}")));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub(super) fn should_reap_idle_worker(last_used: Instant, now: Instant) -> bool {
    now.saturating_duration_since(last_used) >= WORKER_IDLE_TIMEOUT
}

pub(super) fn spawn_worker_idle_reaper(reaper_rx: Receiver<()>, is_claude: bool) {
    std::thread::spawn(move || {
        loop {
            match reaper_rx.recv_timeout(Duration::from_secs(15)) {
                Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => {
                    let now = Instant::now();
                    if is_claude {
                        if let Ok(mut guard) = claude_worker_lock().try_lock() {
                            match guard.as_ref() {
                                None => break,
                                Some(w) if should_reap_idle_worker(w.last_used, now) => {
                                    *guard = None;
                                    break;
                                }
                                Some(_) => {}
                            }
                        }
                    } else if let Ok(mut guard) = agy_worker_lock().try_lock() {
                        match guard.as_ref() {
                            None => break,
                            Some(w) if should_reap_idle_worker(w.last_used, now) => {
                                *guard = None;
                                break;
                            }
                            Some(_) => {}
                        }
                    }
                }
            }
        }
    });
}

#[cfg(windows)]
pub(super) mod win_job {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;

    #[repr(C)]
    struct BasicLimitInfo {
        per_process_user_time_limit: i64,
        per_job_user_time_limit: i64,
        limit_flags: u32,
        minimum_working_set_size: usize,
        maximum_working_set_size: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }

    #[repr(C)]
    struct ExtendedLimitInfo {
        basic: BasicLimitInfo,
        io_info: [u64; 6],
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory_used: usize,
        peak_job_memory_used: usize,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateJobObjectW(attrs: *const c_void, name: *const u16) -> isize;
        fn SetInformationJobObject(job: isize, class: u32, info: *const c_void, len: u32) -> i32;
        fn AssignProcessToJobObject(job: isize, proc_handle: isize) -> i32;
        fn TerminateJobObject(job: isize, exit_code: u32) -> i32;
        fn CloseHandle(handle: isize) -> i32;
    }

    pub struct ProcessJob(isize);

    impl ProcessJob {
        pub fn attach(child: &std::process::Child) -> Option<Self> {
            const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: u32 = 9;
            const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x2000;
            let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if job == 0 {
                return None;
            }
            let mut info: ExtendedLimitInfo = unsafe { std::mem::zeroed() };
            info.basic.limit_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok_info = unsafe {
                SetInformationJobObject(
                    job,
                    JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
                    (&raw const info).cast(),
                    std::mem::size_of::<ExtendedLimitInfo>() as u32,
                )
            };
            if ok_info == 0 || unsafe { AssignProcessToJobObject(job, child.as_raw_handle() as isize) } == 0 {
                unsafe { CloseHandle(job) };
                return None;
            }
            Some(Self(job))
        }

        pub fn terminate(&self) {
            unsafe {
                TerminateJobObject(self.0, 1);
            }
        }
    }

    impl Drop for ProcessJob {
        fn drop(&mut self) {
            unsafe {
                TerminateJobObject(self.0, 1);
                CloseHandle(self.0);
            }
        }
    }
}

#[cfg(not(windows))]
pub(super) mod win_job {
    pub struct ProcessJob;
    impl ProcessJob {
        pub fn attach(_child: &std::process::Child) -> Option<Self> {
            None
        }
        pub fn terminate(&self) {}
    }
}

pub(super) static WORKER_JOBS: OnceLock<Mutex<Vec<Arc<win_job::ProcessJob>>>> = OnceLock::new();

pub(super) fn register_worker_job(child: &std::process::Child) -> Option<Arc<win_job::ProcessJob>> {
    let job = Arc::new(win_job::ProcessJob::attach(child)?);
    if let Ok(mut list) = WORKER_JOBS.get_or_init(|| Mutex::new(Vec::new())).lock() {
        list.retain(|j| Arc::strong_count(j) > 1);
        list.push(Arc::clone(&job));
    }
    Some(job)
}

pub(super) fn spawn_lossy_line_reader(stdout: std::process::ChildStdout, tx: std::sync::mpsc::Sender<String>) {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut buf = Vec::with_capacity(4096);
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    while matches!(buf.last(), Some(b'\n' | b'\r')) {
                        buf.pop();
                    }
                    let line = String::from_utf8_lossy(&buf).into_owned();
                    if tx.send(line).is_err() {
                        break;
                    }
                }
            }
        }
    });
}

pub(super) struct AgyStreamWorker {
    /// `AGY_WORKER_GEN` at spawn; a reset bumps the generation so stale workers are never reused.
    generation: u64,
    model: String,
    /// Effective `--effort` value (`""` = no flag).
    effort: String,
    turns: usize,
    last_used: Instant,
    _reaper_stop: SyncSender<()>,
    _job: Option<Arc<win_job::ProcessJob>>,
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<String>,
}

impl Drop for AgyStreamWorker {
    fn drop(&mut self) {
        if let Some(job) = &self._job {
            job.terminate();
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl AgyStreamWorker {
    fn spawn(model: &str, effort: &str) -> Result<Self, String> {
        let Some(bin_path) = find_cli_binary("agy") else {
            return Err(format!(
                "`agy` CLI를 PATH에서 찾을 수 없습니다. 설치 후 {}",
                ChatProvider::Agy.login_hint()
            ));
        };
        let model_trimmed = model.trim().to_string();
        if !model_trimmed.is_empty() && !is_valid_model_id(&model_trimmed) {
            return Err("유효하지 않은 모델 ID입니다.".to_string());
        }
        let mut args = vec![
            "--disable-slash-commands".to_string(),
            "--input-format".to_string(),
            "stream-json".to_string(),
            "--output-format".to_string(),
            "stream-json".to_string(),
            // Webpage text is untrusted input: read-only plan mode inside agy's terminal sandbox.
            "--mode".to_string(),
            "plan".to_string(),
            "--sandbox".to_string(),
        ];
        let effort_trimmed = effort.trim().to_string();
        if !effort_trimmed.is_empty() && !is_valid_effort(&effort_trimmed) {
            return Err("유효하지 않은 effort 값입니다.".to_string());
        }
        // `--effort` is rejected without an explicit model ("not supported for the current model").
        if !effort_trimmed.is_empty() && !model_trimmed.is_empty() {
            args.push("--effort".to_string());
            args.push(effort_trimmed.clone());
        }
        if !model_trimmed.is_empty() {
            args.push("--model".to_string());
            args.push(model_trimmed.clone());
        }
        let mut cmd = build_cli_command(&bin_path, &args)?;
        cmd.current_dir(empty_cwd("agy-cwd"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("{} 실행 실패: {e}", ChatProvider::Agy.label()))?;
        let _job = register_worker_job(&child);
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "Gemini CLI stdin을 열 수 없습니다.".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "Gemini CLI stdout을 열 수 없습니다.".to_string())?;
        let (tx, rx) = std::sync::mpsc::channel();
        spawn_lossy_line_reader(stdout, tx);
        let (reaper_tx, reaper_rx) = std::sync::mpsc::sync_channel(0);
        spawn_worker_idle_reaper(reaper_rx, false);
        Ok(Self {
            generation: AGY_WORKER_GEN.load(Ordering::Acquire),
            model: model_trimmed,
            effort: effort_trimmed,
            turns: 0,
            last_used: Instant::now(),
            _reaper_stop: reaper_tx,
            _job,
            child,
            stdin,
            rx,
        })
    }

    fn is_alive_for(&mut self, model: &str, effort: &str) -> bool {
        self.generation == AGY_WORKER_GEN.load(Ordering::Acquire)
            && self.model == model.trim()
            && self.effort == effort.trim()
            && self.turns < MAX_CLAUDE_WORKER_TURNS
            && matches!(self.child.try_wait(), Ok(None))
    }

    fn send_turn(
        &mut self,
        prompt: &str,
        cancel: &AtomicBool,
        progress: Option<&super::ProgressCb>,
    ) -> Result<String, String> {
        // Drain any stale lines emitted before sending the new turn.
        while self.rx.try_recv().is_ok() {}
        let payload = json!({
            "event": "user",
            "message": { "content": prompt }
        });
        let line = serde_json::to_string(&payload).map_err(|e| format!("Gemini CLI 요청 직렬화 실패: {e}"))?;
        self.stdin
            .write_all(line.as_bytes())
            .and_then(|()| {
                self.stdin.write_all(b"\n")?;
                self.stdin.flush()
            })
            .map_err(|e| format!("Gemini CLI 입력 전송 실패: {e}"))?;

        let started = Instant::now();
        loop {
            if cancel.load(Ordering::Acquire) {
                return Err("요청이 중지되었습니다.".to_string());
            }
            if started.elapsed() > CLI_TIMEOUT {
                return Err(format!(
                    "{} 응답 시간이 120초를 초과했습니다.",
                    ChatProvider::Agy.label()
                ));
            }
            match self.rx.recv_timeout(Duration::from_millis(50)) {
                Ok(line) => {
                    if let Some(result) = parse_agy_stream_result(&line) {
                        return result;
                    }
                    if let Some(cb) = progress
                        && let Some(p) = super::activity::parse_agy_progress(&line)
                    {
                        cb(p);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    return Err("Gemini CLI 세션이 종료되었습니다.".to_string());
                }
            }
        }
    }
}

pub(super) fn parse_agy_stream_result(line: &str) -> Option<Result<String, String>> {
    let val: Value = serde_json::from_str(line.trim()).ok()?;
    if val.get("event").and_then(|v| v.as_str()) != Some("result") {
        return None;
    }
    let res = val.get("result")?;
    let status = res.get("status").and_then(|v| v.as_str()).unwrap_or("");
    if status.eq_ignore_ascii_case("SUCCESS") {
        let resp = strip_ansi(res.get("response").and_then(|v| v.as_str()).unwrap_or(""))
            .trim()
            .to_string();
        if resp.is_empty() {
            Some(Err(format!("{}가 빈 응답을 반환했습니다.", ChatProvider::Agy.label())))
        } else {
            Some(Ok(resp))
        }
    } else {
        let err = res
            .get("error")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("알 수 없는 오류");
        Some(Err(format!(
            "{} 오류: {}\n\n{}",
            ChatProvider::Agy.label(),
            strip_ansi(err).trim(),
            ChatProvider::Agy.login_hint()
        )))
    }
}

pub(super) static AGY_WORKER: OnceLock<Mutex<Option<AgyStreamWorker>>> = OnceLock::new();
pub(super) static AGY_WORKER_GEN: AtomicU64 = AtomicU64::new(0);

pub(super) fn agy_worker_lock() -> &'static Mutex<Option<AgyStreamWorker>> {
    AGY_WORKER.get_or_init(|| Mutex::new(None))
}

pub fn warm_agy_worker(model: String, effort: String) {
    let epoch = AGY_WORKER_GEN.load(Ordering::Acquire);
    std::thread::spawn(move || {
        if AGY_WORKER_GEN.load(Ordering::Acquire) != epoch {
            return;
        }
        // A turn in flight holds the lock: it already has a live worker, nothing to warm.
        let Ok(mut guard) = agy_worker_lock().try_lock() else {
            return;
        };
        if AGY_WORKER_GEN.load(Ordering::Acquire) != epoch {
            return;
        }
        if let Some(w) = guard.as_mut()
            && w.is_alive_for(&model, &effort)
        {
            w.last_used = Instant::now();
            return;
        }
        if let Ok(worker) = AgyStreamWorker::spawn(&model, &effort)
            && AGY_WORKER_GEN.load(Ordering::Acquire) == epoch
        {
            *guard = Some(worker);
        }
    });
}

/// Retires the current agy worker without blocking the caller (the UI thread): a turn in flight
/// can hold the lock for up to `CLI_TIMEOUT`. Bumping the generation already stops any reuse;
/// the background drop only frees the process, and spares a worker spawned after the reset.
pub fn reset_agy_worker() {
    let generation = AGY_WORKER_GEN.fetch_add(1, Ordering::AcqRel) + 1;
    std::thread::spawn(move || {
        let mut guard = agy_worker_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if guard.as_ref().is_some_and(|w| w.generation < generation) {
            *guard = None;
        }
    });
}

#[allow(clippy::too_many_arguments)] // request plumbing; each argument is one piece of the turn
pub(super) fn run_agy_persistent(
    model: &str,
    effort: &str,
    history: &[ChatMessage],
    user_text: &str,
    page: Option<&PageContext>,
    agent: Option<&AgentContext>,
    cancel: &AtomicBool,
    progress: Option<&super::ProgressCb>,
) -> Result<String, String> {
    let effort = effort.trim();
    let mut guard = agy_worker_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let was_reused = guard.as_mut().is_some_and(|w| w.is_alive_for(model, effort));
    if !was_reused {
        *guard = Some(AgyStreamWorker::spawn(model, effort)?);
    }
    let include_history = guard.as_ref().is_none_or(|w| w.turns == 0);
    let mut prompt = build_cli_prompt(history, user_text, page, agent, include_history);
    // agy has no image flag, but its sandboxed `read_file` reads images: drop the agent's
    // screenshot into its working folder and point it there.
    if let Some(bytes) = agent
        .and_then(|a| a.image_png_b64.as_deref())
        .and_then(super::decode_base64)
    {
        let file = empty_cwd("agy-cwd").join("agent-screenshot.png");
        if std::fs::write(&file, bytes).is_ok() {
            prompt.push_str(&format!(
                "

[SCREENSHOT] The visible page was captured to `{}`. Open it with read_file before answering.",
                file.display()
            ));
        }
    }
    let res = match guard.as_mut() {
        Some(w) => w.send_turn(&prompt, cancel, progress),
        None => return Err("Gemini CLI 워커 초기화 실패".to_string()),
    };
    match res {
        Ok(text) => {
            let needs_rotate = if let Some(w) = guard.as_mut() {
                w.turns += 1;
                w.last_used = Instant::now();
                w.turns >= MAX_CLAUDE_WORKER_TURNS
            } else {
                false
            };
            if needs_rotate {
                *guard = None;
                drop(guard);
                warm_agy_worker(model.to_string(), effort.to_string());
            }
            Ok(text)
        }
        Err(err) => {
            *guard = None;
            let broken = err.contains("입력 전송 실패") || err.contains("세션이 종료되었습니다");
            if was_reused && broken && !cancel.load(Ordering::Acquire) {
                let fresh_prompt = build_cli_prompt(history, user_text, page, agent, true);
                let mut fresh = AgyStreamWorker::spawn(model, effort)?;
                let retry = fresh.send_turn(&fresh_prompt, cancel, progress);
                if retry.is_ok() {
                    fresh.turns = 1;
                    fresh.last_used = Instant::now();
                    *guard = Some(fresh);
                }
                retry
            } else {
                Err(err)
            }
        }
    }
}

/// Writes the system prompt next to the empty cwd and returns its path, for `--system-prompt-file`:
/// multi-line text can't travel as an argument through a `claude.cmd` shim (cmd.exe stops at `\n`).
pub(super) fn claude_system_prompt_file(agent_mode: bool) -> Result<PathBuf, String> {
    let dir = empty_cwd("claude-prompt");
    let pid = std::process::id();
    let name = if agent_mode {
        format!("system-agent-{pid}.txt")
    } else {
        format!("system-{pid}.txt")
    };
    let path = dir.join(name);
    std::fs::write(&path, claude_system_prompt(agent_mode))
        .map_err(|e| format!("Claude 시스템 프롬프트 파일 쓰기 실패: {e}"))?;
    Ok(path)
}

pub(super) fn resolve_claude_binary() -> Option<PathBuf> {
    let bin = find_cli_binary("claude")?;
    #[cfg(windows)]
    {
        let is_shim = bin
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"));
        if is_shim && let Some(parent) = bin.parent() {
            let direct = parent
                .join("node_modules")
                .join("@anthropic-ai")
                .join("claude-code")
                .join("bin")
                .join("claude.exe");
            if direct.is_file() {
                return Some(direct);
            }
        }
    }
    Some(bin)
}

pub(super) struct ClaudeStreamWorker {
    /// `CLAUDE_WORKER_GEN` at spawn (see `AgyStreamWorker::generation`).
    generation: u64,
    model: String,
    /// Effective `--effort` value (`""` = no flag).
    effort: String,
    /// Agent-control mode: selects the strict control system prompt file.
    agent_mode: bool,
    turns: usize,
    last_used: Instant,
    _reaper_stop: SyncSender<()>,
    _job: Option<Arc<win_job::ProcessJob>>,
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<String>,
    stderr_buf: Arc<Mutex<String>>,
}

impl Drop for ClaudeStreamWorker {
    fn drop(&mut self) {
        if let Some(job) = &self._job {
            job.terminate();
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl ClaudeStreamWorker {
    fn spawn(model: &str, effort: &str, agent_mode: bool) -> Result<Self, String> {
        let Some(bin_path) = resolve_claude_binary() else {
            return Err(format!(
                "`claude` CLI를 PATH에서 찾을 수 없습니다. 설치 후 {}",
                ChatProvider::Claude.login_hint()
            ));
        };
        let model_trimmed = model.trim().to_string();
        if !model_trimmed.is_empty() && !is_valid_model_id(&model_trimmed) {
            return Err("유효하지 않은 모델 ID입니다.".to_string());
        }
        let effort_trimmed = effort.trim().to_string();
        if !effort_trimmed.is_empty() && !is_valid_effort(&effort_trimmed) {
            return Err("유효하지 않은 effort 값입니다.".to_string());
        }
        // The system prompt variant tracks agent-control mode (passed separately from
        // the user-adjustable effort), so a normal chat never gets control instructions.
        let sys_prompt_file = claude_system_prompt_file(agent_mode)?;
        let mut args = vec![
            "-p".to_string(),
            "--safe-mode".to_string(),
            "--tools".to_string(),
            String::new(),
            "--no-session-persistence".to_string(),
            "--disable-slash-commands".to_string(),
            "--strict-mcp-config".to_string(),
            "--no-chrome".to_string(),
            "--prompt-suggestions".to_string(),
            "false".to_string(),
            "--input-format".to_string(),
            "stream-json".to_string(),
            "--output-format".to_string(),
            "stream-json".to_string(),
            "--verbose".to_string(),
            "--system-prompt-file".to_string(),
            sys_prompt_file.display().to_string(),
        ];
        if !effort_trimmed.is_empty() {
            args.push("--effort".to_string());
            args.push(effort_trimmed.clone());
        }
        if !model_trimmed.is_empty() {
            args.push("--model".to_string());
            args.push(model_trimmed.clone());
        }
        let mut cmd = build_cli_command(&bin_path, &args)?;
        cmd.current_dir(empty_cwd("claude-cwd"))
            .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
            .env("DISABLE_TELEMETRY", "1")
            .env("DISABLE_AUTOUPDATER", "1")
            .env("DISABLE_ERROR_REPORTING", "1")
            .env("CLAUDE_CODE_DISABLE_FEEDBACK_SURVEY", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if effort_trimmed == "low" {
            cmd.env("MAX_THINKING_TOKENS", "0");
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("{} 실행 실패: {e}", ChatProvider::Claude.label()))?;
        let _job = register_worker_job(&child);
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "Claude CLI stdin을 열 수 없습니다.".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "Claude CLI stdout을 열 수 없습니다.".to_string())?;
        let stderr = child.stderr.take();
        let stderr_buf = Arc::new(Mutex::new(String::new()));
        if let Some(err_pipe) = stderr {
            let err_dst = Arc::clone(&stderr_buf);
            std::thread::spawn(move || {
                let text = read_pipe_to_string(Some(err_pipe));
                if let Ok(mut guard) = err_dst.lock() {
                    *guard = text.chars().take(2048).collect();
                }
            });
        }
        let (tx, rx) = std::sync::mpsc::channel();
        spawn_lossy_line_reader(stdout, tx);
        let (reaper_tx, reaper_rx) = std::sync::mpsc::sync_channel(0);
        spawn_worker_idle_reaper(reaper_rx, true);
        Ok(Self {
            generation: CLAUDE_WORKER_GEN.load(Ordering::Acquire),
            model: model_trimmed,
            effort: effort_trimmed,
            agent_mode,
            turns: 0,
            last_used: Instant::now(),
            _reaper_stop: reaper_tx,
            _job,
            child,
            stdin,
            rx,
            stderr_buf,
        })
    }

    fn is_alive_for(&mut self, model: &str, effort: &str, agent_mode: bool) -> bool {
        self.generation == CLAUDE_WORKER_GEN.load(Ordering::Acquire)
            && self.model == model.trim()
            && self.effort == effort.trim()
            && self.agent_mode == agent_mode
            && self.turns < MAX_CLAUDE_WORKER_TURNS
            && matches!(self.child.try_wait(), Ok(None))
    }

    fn send_turn(
        &mut self,
        prompt: &str,
        cancel: &AtomicBool,
        progress: Option<&super::ProgressCb>,
        image: Option<&str>,
    ) -> Result<String, String> {
        while self.rx.try_recv().is_ok() {}
        let payload = json!({
            "type": "user",
            "message": { "role": "user", "content": match image {
                Some(b64) => json!([
                    { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": b64 } },
                    { "type": "text", "text": prompt },
                ]),
                None => json!(prompt),
            }}
        });
        let line = serde_json::to_string(&payload).map_err(|e| format!("Claude CLI 요청 직렬화 실패: {e}"))?;
        self.stdin
            .write_all(line.as_bytes())
            .and_then(|()| {
                self.stdin.write_all(b"\n")?;
                self.stdin.flush()
            })
            .map_err(|e| format!("Claude CLI 입력 전송 실패: {e}"))?;

        let started = Instant::now();
        loop {
            if cancel.load(Ordering::Acquire) {
                return Err("요청이 중지되었습니다.".to_string());
            }
            if started.elapsed() > CLI_TIMEOUT {
                return Err(format!(
                    "{} 응답 시간이 120초를 초과했습니다.",
                    ChatProvider::Claude.label()
                ));
            }
            match self.rx.recv_timeout(Duration::from_millis(50)) {
                Ok(line) => {
                    if let Some(result) = parse_claude_stream_result(&line) {
                        return result;
                    }
                    if let Some(cb) = progress {
                        super::activity::parse_claude_progress(&line)
                            .into_iter()
                            .for_each(|p| cb(p));
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    let detail = self
                        .stderr_buf
                        .lock()
                        .ok()
                        .map(|s| strip_ansi(&s).trim().to_string())
                        .unwrap_or_default();
                    return if detail.is_empty() {
                        Err(format!(
                            "Claude CLI 세션이 종료되었습니다.\n\n{}",
                            ChatProvider::Claude.login_hint()
                        ))
                    } else {
                        let preview: String = detail.chars().take(360).collect();
                        Err(format!(
                            "Claude CLI 세션이 종료되었습니다: {preview}\n\n{}",
                            ChatProvider::Claude.login_hint()
                        ))
                    };
                }
            }
        }
    }
}

pub(super) fn parse_claude_stream_result(line: &str) -> Option<Result<String, String>> {
    let val: Value = serde_json::from_str(line.trim()).ok()?;
    if val.get("type").and_then(|v| v.as_str()) != Some("result") {
        return None;
    }
    let is_error = val.get("is_error").and_then(|v| v.as_bool()).unwrap_or(false);
    let subtype = val.get("subtype").and_then(|v| v.as_str()).unwrap_or("");
    if is_error || subtype.starts_with("error") {
        let msg = val
            .get("result")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .or_else(|| {
                val.get("errors")
                    .and_then(|v| v.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.trim().is_empty())
            })
            .or_else(|| (!subtype.is_empty()).then_some(subtype))
            .unwrap_or("알 수 없는 오류");
        Some(Err(format!(
            "{} 오류: {}\n\n{}",
            ChatProvider::Claude.label(),
            strip_ansi(msg).trim(),
            ChatProvider::Claude.login_hint()
        )))
    } else {
        let resp = strip_ansi(val.get("result").and_then(|v| v.as_str()).unwrap_or(""))
            .trim()
            .to_string();
        if resp.is_empty() {
            Some(Err("Claude CLI가 빈 응답을 반환했습니다.".to_string()))
        } else {
            Some(Ok(resp))
        }
    }
}

pub(super) static CLAUDE_WORKER: OnceLock<Mutex<Option<ClaudeStreamWorker>>> = OnceLock::new();
pub(super) static CLAUDE_WORKER_GEN: AtomicU64 = AtomicU64::new(0);

pub(super) fn claude_worker_lock() -> &'static Mutex<Option<ClaudeStreamWorker>> {
    CLAUDE_WORKER.get_or_init(|| Mutex::new(None))
}

pub fn warm_claude_worker(model: String, effort: String, agent_mode: bool) {
    let epoch = CLAUDE_WORKER_GEN.load(Ordering::Acquire);
    std::thread::spawn(move || {
        if CLAUDE_WORKER_GEN.load(Ordering::Acquire) != epoch {
            return;
        }
        let Ok(mut guard) = claude_worker_lock().try_lock() else {
            return;
        };
        if CLAUDE_WORKER_GEN.load(Ordering::Acquire) != epoch {
            return;
        }
        if let Some(w) = guard.as_mut()
            && w.is_alive_for(&model, &effort, agent_mode)
        {
            w.last_used = Instant::now();
            return;
        }
        if let Ok(worker) = ClaudeStreamWorker::spawn(&model, &effort, agent_mode)
            && CLAUDE_WORKER_GEN.load(Ordering::Acquire) == epoch
        {
            *guard = Some(worker);
        }
    });
}

/// Non-blocking, like `reset_agy_worker`.
pub fn reset_claude_worker() {
    let generation = CLAUDE_WORKER_GEN.fetch_add(1, Ordering::AcqRel) + 1;
    std::thread::spawn(move || {
        let mut guard = claude_worker_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if guard.as_ref().is_some_and(|w| w.generation < generation) {
            *guard = None;
        }
    });
}

/// Non-blocking reset of both persistent CLI workers.
pub fn reset_all_cli_workers() {
    reset_agy_worker();
    reset_claude_worker();
}

/// Non-blocking termination + bounded cleanup for app exit, so worker processes are killed
/// without ever deadlocking the UI thread on a blocked stdin write.
pub fn shutdown_cli_workers() {
    AGY_WORKER_GEN.fetch_add(1, Ordering::AcqRel);
    CLAUDE_WORKER_GEN.fetch_add(1, Ordering::AcqRel);
    if let Ok(mut jobs) = WORKER_JOBS.get_or_init(|| Mutex::new(Vec::new())).lock() {
        for job in jobs.drain(..) {
            job.terminate();
        }
    }
    let deadline = Instant::now() + Duration::from_millis(250);
    loop {
        if let Ok(mut g) = agy_worker_lock().try_lock() {
            *g = None;
            break;
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    loop {
        if let Ok(mut g) = claude_worker_lock().try_lock() {
            *g = None;
            break;
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let prompt_dir = empty_cwd("claude-prompt");
    let pid = std::process::id();
    let _ = std::fs::remove_file(prompt_dir.join(format!("system-{pid}.txt")));
    let _ = std::fs::remove_file(prompt_dir.join(format!("system-agent-{pid}.txt")));
}

#[allow(clippy::too_many_arguments)] // request plumbing; each argument is one piece of the turn
pub(super) fn run_claude_persistent(
    model: &str,
    effort: &str,
    history: &[ChatMessage],
    user_text: &str,
    page: Option<&PageContext>,
    agent: Option<&AgentContext>,
    cancel: &AtomicBool,
    progress: Option<&super::ProgressCb>,
) -> Result<String, String> {
    let effort = effort.trim();
    let agent_mode = agent.is_some();
    let image = agent.and_then(|a| a.image_png_b64.as_deref());
    let mut guard = claude_worker_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let was_reused = guard
        .as_mut()
        .is_some_and(|w| w.is_alive_for(model, effort, agent_mode));
    if !was_reused {
        *guard = Some(ClaudeStreamWorker::spawn(model, effort, agent_mode)?);
    }
    let include_history = guard.as_ref().is_none_or(|w| w.turns == 0);
    let prompt = build_claude_turn_prompt(history, user_text, page, agent, include_history);
    let res = match guard.as_mut() {
        Some(w) => w.send_turn(&prompt, cancel, progress, image),
        None => return Err("Claude CLI 워커 초기화 실패".to_string()),
    };
    match res {
        Ok(text) => {
            let needs_rotate = if let Some(w) = guard.as_mut() {
                w.turns += 1;
                w.last_used = Instant::now();
                w.turns >= MAX_CLAUDE_WORKER_TURNS
            } else {
                false
            };
            if needs_rotate {
                *guard = None;
                drop(guard);
                warm_claude_worker(model.to_string(), effort.to_string(), agent_mode);
            }
            Ok(text)
        }
        Err(err) => {
            *guard = None;
            let broken = err.contains("입력 전송 실패") || err.contains("세션이 종료되었습니다");
            if was_reused && broken && !cancel.load(Ordering::Acquire) {
                let mut fresh = ClaudeStreamWorker::spawn(model, effort, agent_mode)?;
                let fresh_prompt = build_claude_turn_prompt(history, user_text, page, agent, true);
                let retry = fresh.send_turn(&fresh_prompt, cancel, progress, image);
                if retry.is_ok() {
                    fresh.turns = 1;
                    fresh.last_used = Instant::now();
                    *guard = Some(fresh);
                }
                retry
            } else {
                Err(err)
            }
        }
    }
}

pub(super) fn find_cli_binary(name: &str) -> Option<PathBuf> {
    #[cfg_attr(not(windows), allow(unused_mut))] // only mutated by the Windows block below
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|v| std::env::split_paths(&v).collect())
        .unwrap_or_default();
    #[cfg(windows)]
    {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            dirs.push(PathBuf::from(appdata).join("npm"));
        }
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            dirs.push(PathBuf::from(local).join("agy").join("bin"));
        }
    }
    for dir in dirs {
        if dir.as_os_str().is_empty() || !dir.is_absolute() {
            continue;
        }
        #[cfg(windows)]
        for ext in ["exe", "cmd", "bat"] {
            let candidate = dir.join(format!("{name}.{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        let plain = dir.join(name);
        if plain.is_file() {
            return Some(plain);
        }
    }
    None
}

/// `cmd.exe /C shim.cmd args…` re-parses the command line with cmd's own rules: a newline ends the
/// command and quotes/`%`/`&|<>^!` are interpreted, so such args would be truncated or injected.
pub(super) fn arg_unsafe_for_cmd(arg: &str) -> bool {
    arg.chars().any(|c| {
        matches!(
            c,
            '\r' | '\n' | '"' | '%' | '^' | '&' | '|' | '<' | '>' | '!' | '(' | ')' | ';' | ',' | '='
        )
    })
}

pub(super) fn build_cli_command(bin_path: &Path, args: &[String]) -> Result<Command, String> {
    let is_cmd_shim = bin_path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"));
    if cfg!(windows)
        && is_cmd_shim
        && (arg_unsafe_for_cmd(&bin_path.to_string_lossy()) || args.iter().any(|a| arg_unsafe_for_cmd(a)))
    {
        return Err("CLI 인자에 cmd.exe가 해석하는 특수문자(\" % & | < > ^ ! ( ) ; , = 또는 줄바꿈)가 있어 실행하지 않았습니다. 모델 ID를 확인하세요.".to_string());
    }

    #[cfg_attr(not(windows), allow(unused_mut))] // only mutated by the Windows block below
    let mut cmd = if cfg!(windows) && is_cmd_shim {
        let mut c = Command::new("cmd.exe");
        c.arg("/C").arg(bin_path).args(args);
        c
    } else {
        let mut c = Command::new(bin_path);
        c.args(args);
        c
    };

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    Ok(cmd)
}

pub(super) fn run_cli_agent(
    provider: ChatProvider,
    model: &str,
    effort: &str,
    prompt: &str,
    image_png_b64: Option<&str>,
    cancel: &AtomicBool,
) -> Result<String, String> {
    let bin_name = match provider {
        ChatProvider::Gemini => "gemini",
        ChatProvider::Codex => "codex",
        ChatProvider::Muse => "muse",
        _ => return Err("지원되지 않는 CLI 에이전트입니다.".to_string()),
    };
    let Some(bin_path) = find_cli_binary(bin_name) else {
        return Err(format!(
            "`{bin_name}` CLI를 PATH에서 찾을 수 없습니다. 설치 후 {}",
            provider.login_hint()
        ));
    };

    let model = model.trim();
    if !model.is_empty() && !is_valid_model_id(model) {
        return Err("유효하지 않은 모델 ID입니다.".to_string());
    }
    let mut args: Vec<String> = match provider {
        ChatProvider::Gemini => vec!["-p".into(), "".into(), "--approval-mode".into(), "plan".into()],
        ChatProvider::Codex => vec!["exec".into(), "--sandbox".into(), "read-only".into()],
        // `--preset empty`: plain chat, no workspace tools. Effort defaults to low: Muse's
        // own default (high) makes a short browser question take tens of seconds.
        ChatProvider::Muse => vec![
            "exec".into(),
            "--preset".into(),
            "empty".into(),
            "--reasoning-effort".into(),
            if is_valid_effort(effort) && !effort.is_empty() {
                effort.into()
            } else {
                "low".into()
            },
        ],
        _ => Vec::new(),
    };
    // Muse gets its own dir: the prompt file must never sit in Gemini/Codex's working directory.
    let cwd = empty_cwd(if provider == ChatProvider::Muse {
        "muse-cwd"
    } else {
        "cli-cwd"
    });
    if provider == ChatProvider::Muse {
        // Muse reads the prompt from an argument or file, not stdin.
        let file = cwd.join("muse-prompt.txt");
        std::fs::write(&file, prompt).map_err(|e| format!("프롬프트 파일 쓰기 실패: {e}"))?;
        args.push("--prompt-file".into());
        args.push(file.to_string_lossy().into_owned());
    }
    // The agent's screenshot: Codex and Muse both take `--image <file>`.
    let image_file = cwd.join("agent-screenshot.png");
    if let Some(b64) = image_png_b64.filter(|_| matches!(provider, ChatProvider::Codex | ChatProvider::Muse))
        && let Some(bytes) = super::decode_base64(b64)
        && std::fs::write(&image_file, bytes).is_ok()
    {
        args.push("--image".into());
        args.push(image_file.to_string_lossy().into_owned());
    }
    if !model.is_empty() {
        args.push("--model".into());
        args.push(model.to_string());
    }
    if provider == ChatProvider::Codex {
        args.push("-".into());
    }

    let mut cmd = build_cli_command(&bin_path, &args)?;
    // Gemini/Codex read files in their working directory: start them in an empty one.
    cmd.current_dir(&cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("{} 실행 실패: {e}", provider.label()))?;
    let job = win_job::ProcessJob::attach(&child);

    let mut stdin = child.stdin.take();
    let prompt_bytes = prompt.as_bytes().to_vec();
    let writer = std::thread::spawn(move || {
        if let Some(mut s) = stdin.take() {
            let _ = s.write_all(&prompt_bytes);
        }
    });

    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();
    let out_reader = std::thread::spawn(move || read_pipe_to_string(stdout_pipe));
    let err_reader = std::thread::spawn(move || read_pipe_to_string(stderr_pipe));

    let started = Instant::now();
    let status = loop {
        if cancel.load(Ordering::Acquire) {
            if let Some(j) = &job {
                j.terminate();
            }
            let _ = child.kill();
            let _ = child.wait();
            drop(job);
            return Err("요청이 중지되었습니다.".to_string());
        }
        if started.elapsed() > CLI_TIMEOUT {
            if let Some(j) = &job {
                j.terminate();
            }
            let _ = child.kill();
            let _ = child.wait();
            drop(job);
            return Err(format!("{} 응답 시간이 120초를 초과했습니다.", provider.label()));
        }
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => {
                if let Some(j) = &job {
                    j.terminate();
                }
                let _ = child.kill();
                drop(job);
                return Err(format!("{} 프로세스 상태 확인 실패: {e}", provider.label()));
            }
        }
    };
    drop(job);

    let _ = writer.join();
    let _ = std::fs::remove_file(cwd.join("muse-prompt.txt"));
    let _ = std::fs::remove_file(&image_file);
    let stdout = strip_ansi(&out_reader.join().unwrap_or_default()).trim().to_string();
    let stderr = strip_ansi(&err_reader.join().unwrap_or_default()).trim().to_string();

    if status.success() {
        if stdout.is_empty() {
            Err(format!("{}가 빈 응답을 반환했습니다.", provider.label()))
        } else {
            Ok(stdout)
        }
    } else {
        let detail = if !stderr.is_empty() { &stderr } else { &stdout };
        let short_detail: String = detail.chars().take(400).collect();
        if short_detail.is_empty() {
            Err(format!(
                "{} 실행 실패 (종료 코드 {:?}). {}",
                provider.label(),
                status.code(),
                provider.login_hint()
            ))
        } else {
            Err(format!(
                "{} 오류: {}\n\n{}",
                provider.label(),
                short_detail,
                provider.login_hint()
            ))
        }
    }
}

pub(super) fn read_pipe_to_string<R: std::io::Read>(pipe: Option<R>) -> String {
    let Some(mut r) = pipe else {
        return String::new();
    };
    let mut buf = Vec::new();
    let _ = r.read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).into_owned()
}

pub fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            match chars.peek().copied() {
                Some('[') => {
                    chars.next();
                    for nc in chars.by_ref() {
                        if ('@'..='~').contains(&nc) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    while let Some(nc) = chars.next() {
                        if nc == '\x07' || (nc == '\x1b' && chars.peek() == Some(&'\\')) {
                            if nc == '\x1b' {
                                chars.next();
                            }
                            break;
                        }
                    }
                }
                _ => {}
            }
        } else if c != '\r' {
            out.push(c);
        }
    }
    out
}
