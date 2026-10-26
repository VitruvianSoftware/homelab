// Copyright (c) 2026 VitruvianSoftware
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

//! Package remote provides an SSH-based executor for running commands on
//! remote macOS hosts. It shells out to the system ssh binary to leverage
//! the user's existing SSH config and agent.

use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crate::logging;

/// DefaultMaxRetries is the default number of retries for transient SSH failures.
pub const DEFAULT_MAX_RETRIES: usize = 3;

/// Context models Go's `context.Context` with cancellation and optional deadline.
#[derive(Clone)]
pub struct Context {
    cancelled: Arc<AtomicBool>,
    deadline: Option<Instant>,
}

impl Default for Context {
    fn default() -> Self {
        Self::background()
    }
}

impl Context {
    /// Creates a new non-cancelled background context.
    pub fn background() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            deadline: None,
        }
    }

    /// Creates a context with a timeout duration.
    pub fn with_timeout(timeout: Duration) -> (Self, Arc<AtomicBool>) {
        let flag = Arc::new(AtomicBool::new(false));
        let ctx = Self {
            cancelled: Arc::clone(&flag),
            deadline: Some(Instant::now() + timeout),
        };
        (ctx, flag)
    }

    /// Creates a cancellable context and returns the context plus a cancel handle.
    pub fn with_cancel() -> (Self, CancelFn) {
        let flag = Arc::new(AtomicBool::new(false));
        let ctx = Self {
            cancelled: Arc::clone(&flag),
            deadline: None,
        };
        (ctx, CancelFn(flag))
    }

    /// Returns `Some("context canceled")` or `Some("context deadline exceeded")` if done.
    pub fn err(&self) -> Option<&'static str> {
        if self.cancelled.load(Ordering::SeqCst) {
            return Some("context canceled");
        }
        if let Some(dl) = self.deadline {
            if Instant::now() >= dl {
                return Some("context deadline exceeded");
            }
        }
        None
    }

    /// Sleeps for `dur` unless the context is cancelled first.
    pub fn sleep(&self, dur: Duration) -> Result<(), &'static str> {
        if let Some(e) = self.err() {
            return Err(e);
        }
        if dur.is_zero() {
            return Ok(());
        }
        let step = Duration::from_millis(25);
        let start = Instant::now();
        while start.elapsed() < dur {
            if let Some(e) = self.err() {
                return Err(e);
            }
            let remaining = dur.saturating_sub(start.elapsed());
            thread::sleep(remaining.min(step));
        }
        if let Some(e) = self.err() {
            return Err(e);
        }
        Ok(())
    }
}

/// Handle that cancels a `Context` when invoked.
#[derive(Clone)]
pub struct CancelFn(Arc<AtomicBool>);

impl CancelFn {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// Custom executor callback type for testing SSH operations without network access.
pub type ExecutorFn = dyn Fn(&str, &str) -> Result<String, String> + Send + Sync;

/// Runner executes commands on a remote host via SSH.
#[derive(Clone)]
pub struct Runner {
    pub host: String,
    pub user: String,
    pub port: String,
    pub key_path: String,
    pub max_retries: usize,
    pub backoff_base: Duration,
    executor: Option<Arc<ExecutorFn>>,
}

impl std::fmt::Debug for Runner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Runner")
            .field("host", &self.host)
            .field("user", &self.user)
            .field("port", &self.port)
            .field("key_path", &self.key_path)
            .field("max_retries", &self.max_retries)
            .finish()
    }
}

impl Runner {
    /// Creates a new remote command runner for the given SSH host.
    pub fn new(host: &str) -> Self {
        Self {
            host: host.to_string(),
            user: String::new(),
            port: String::new(),
            key_path: String::new(),
            max_retries: DEFAULT_MAX_RETRIES,
            backoff_base: Duration::from_secs(1),
            executor: None,
        }
    }

    /// Creates a runner with custom SSH options.
    pub fn with_opts(host: &str, user: &str, port: &str, key_path: &str) -> Self {
        let mut r = Self::new(host);
        r.user = user.to_string();
        r.port = port.to_string();
        r.key_path = key_path.to_string();
        r
    }

    /// Attaches a mock executor for unit testing.
    pub fn with_executor<F>(mut self, exec: F) -> Self
    where
        F: Fn(&str, &str) -> Result<String, String> + Send + Sync + 'static,
    {
        self.executor = Some(Arc::new(exec));
        self
    }

    /// Attaches a shared mock executor for unit testing.
    pub fn with_shared_executor(mut self, exec: Arc<ExecutorFn>) -> Self {
        self.executor = Some(exec);
        self
    }

    /// Builds the `ssh` CLI arguments for a remote command.
    pub fn build_ssh_args(&self, command: &str) -> Vec<String> {
        let mut args = vec![
            "-o".to_string(),
            "BatchMode=yes".to_string(),
            "-o".to_string(),
            "ConnectTimeout=10".to_string(),
            "-o".to_string(),
            "StrictHostKeyChecking=accept-new".to_string(),
        ];

        if !self.port.is_empty() {
            args.push("-p".to_string());
            args.push(self.port.clone());
        }
        if !self.key_path.is_empty() {
            args.push("-i".to_string());
            args.push(self.key_path.clone());
        }

        let target = if self.user.is_empty() {
            self.host.clone()
        } else {
            format!("{}@{}", self.user, self.host)
        };

        let wrapped_cmd = format!(
            "export PATH=/opt/homebrew/bin:/usr/local/bin:$PATH; {}",
            command
        );
        args.push(target);
        args.push(wrapped_cmd);
        args
    }

    /// Executes a command on the remote host and returns the combined output.
    /// Retries on transient failures with exponential backoff.
    pub fn run(&self, ctx: &Context, command: &str) -> Result<String, String> {
        let mut last_err = String::new();

        for attempt in 0..=self.max_retries {
            if attempt > 0 {
                let multiplier = 1u64 << (attempt - 1);
                let backoff = self.backoff_base.saturating_mul(multiplier as u32);
                logging::debug(
                    "retrying SSH command",
                    &[
                        ("host", &self.host),
                        ("attempt", &(attempt + 1).to_string()),
                    ],
                );
                if let Err(ctx_err) = ctx.sleep(backoff) {
                    return Err(format!(
                        "ssh {}: {} (after {} attempts, last error: {})",
                        self.host, ctx_err, attempt, last_err
                    ));
                }
            }

            if let Some(ctx_err) = ctx.err() {
                return Err(format!("ssh {}: {}", self.host, ctx_err));
            }

            match self.run_once(ctx, command) {
                Ok(output) => return Ok(output),
                Err(err) => {
                    last_err = err.clone();
                    if let Some(ctx_err) = ctx.err() {
                        return Err(format!("ssh {}: {}", self.host, ctx_err));
                    }
                    if !is_transient(&err) {
                        return Err(err);
                    }
                    logging::warn(
                        "transient SSH failure",
                        &[
                            ("host", &self.host),
                            ("attempt", &(attempt + 1).to_string()),
                            ("error", &err),
                        ],
                    );
                }
            }
        }

        Err(format!(
            "ssh {}: exhausted {} retries, last error: {}",
            self.host, self.max_retries, last_err
        ))
    }

    fn run_once(&self, ctx: &Context, command: &str) -> Result<String, String> {
        if let Some(ctx_err) = ctx.err() {
            return Err(format!("ssh {}: {}", self.host, ctx_err));
        }

        if let Some(ref exec) = self.executor {
            return exec(&self.host, command).map(|s| s.trim().to_string());
        }

        let args = self.build_ssh_args(command);
        logging::debug(
            "executing SSH command",
            &[("host", &self.host), ("command", command)],
        );

        let output = Command::new("ssh")
            .args(&args)
            .output()
            .map_err(|e| format!("ssh {}: {}", self.host, e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(format!(
                "ssh {}: {}\nstderr: {}",
                self.host, output.status, stderr
            ));
        }

        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        logging::debug(
            "SSH command completed",
            &[
                ("host", &self.host),
                ("output_length", &stdout.len().to_string()),
            ],
        );
        Ok(stdout)
    }

    /// Executes a command via `sh -c` on the remote host.
    pub fn run_shell(&self, ctx: &Context, script: &str) -> Result<String, String> {
        self.run(ctx, &format!("sh -c {}", go_quote(script)))
    }

    /// Executes a command inside a Lima VM on the remote host.
    pub fn lima_shell(
        &self,
        ctx: &Context,
        vm_name: &str,
        command: &str,
    ) -> Result<String, String> {
        self.run(ctx, &format!("limactl shell {} -- {}", vm_name, command))
    }

    /// Executes a command as root inside a Lima VM on the remote host.
    pub fn lima_shell_sudo(
        &self,
        ctx: &Context,
        vm_name: &str,
        command: &str,
    ) -> Result<String, String> {
        self.run(
            ctx,
            &format!(
                "limactl shell {} -- sudo sh -c {}",
                vm_name,
                go_quote(command)
            ),
        )
    }
}

/// Formats a string as a double-quoted Go `%q` literal.
pub fn go_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Returns true if the error message suggests a transient SSH failure.
pub fn is_transient(err_msg: &str) -> bool {
    const TRANSIENT_PATTERNS: &[&str] = &[
        "Connection refused",
        "Connection reset",
        "Connection timed out",
        "No route to host",
        "ssh_exchange_identification",
        "kex_exchange_identification",
        "Host is down",
        "Network is unreachable",
    ];
    TRANSIENT_PATTERNS.iter().any(|p| err_msg.contains(p))
}
