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

//! Package logging provides a structured logger for the homelab CLI.

use std::sync::atomic::{AtomicBool, Ordering};

static VERBOSE: AtomicBool = AtomicBool::new(false);

/// Initializes the global logger level (`INFO` by default, `DEBUG` when `verbose` is true).
pub fn setup(verbose: bool) {
    VERBOSE.store(verbose, Ordering::SeqCst);
}

/// Returns whether debug logging is currently enabled.
pub fn is_verbose() -> bool {
    VERBOSE.load(Ordering::SeqCst)
}

/// Formats a structured log line with `level`, `msg`, and key-value pairs.
pub fn format_log(level: &str, msg: &str, fields: &[(&str, &str)]) -> String {
    let mut out = format!("level={} msg={:?}", level, msg);
    for (k, v) in fields {
        if v.contains(' ') || v.is_empty() {
            out.push_str(&format!(" {}={:?}", k, v));
        } else {
            out.push_str(&format!(" {}={}", k, v));
        }
    }
    out
}

/// Logs an INFO message to stderr.
pub fn info(msg: &str, fields: &[(&str, &str)]) {
    eprintln!("{}", format_log("INFO", msg, fields));
}

/// Logs a DEBUG message to stderr if verbose mode is enabled.
pub fn debug(msg: &str, fields: &[(&str, &str)]) {
    if is_verbose() {
        eprintln!("{}", format_log("DEBUG", msg, fields));
    }
}

/// Logs a WARN message to stderr.
pub fn warn(msg: &str, fields: &[(&str, &str)]) {
    eprintln!("{}", format_log("WARN", msg, fields));
}

/// Logs an ERROR message to stderr.
pub fn error(msg: &str, fields: &[(&str, &str)]) {
    eprintln!("{}", format_log("ERROR", msg, fields));
}
