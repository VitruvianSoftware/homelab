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

//! Package prereqs checks and optionally installs prerequisites on remote macOS hosts.

use crate::config::NodeConfig;
use crate::lima::base64_encode;
use crate::logging;
use crate::remote::{Context, Runner};

/// Shell command to detect the full path to `brew` across ARM64 and Intel macOS hosts.
pub const BREW_DETECT: &str = r#"if [ -x /opt/homebrew/bin/brew ]; then echo /opt/homebrew/bin/brew; elif [ -x /usr/local/bin/brew ]; then echo /usr/local/bin/brew; else echo ""; fi"#;

/// CheckResult holds the result of a single prerequisite check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckResult {
    pub name: String,
    pub installed: bool,
    pub message: String,
}

/// Verifies and optionally installs prerequisites on a single host.
pub fn ensure_all(ctx: &Context, node: &NodeConfig, auto_install: bool) -> Result<(), String> {
    let runner = new_runner(node);
    ensure_all_with_runner(ctx, &runner, &node.host, auto_install)
}

/// Verifies and optionally installs prerequisites using a provided `Runner`.
pub fn ensure_all_with_runner(
    ctx: &Context,
    runner: &Runner,
    host: &str,
    auto_install: bool,
) -> Result<(), String> {
    logging::info(
        "checking prerequisites",
        &[("host", host), ("auto_install", &auto_install.to_string())],
    );
    println!("  [{}] Checking prerequisites...", host);

    let brew_path = detect_or_install_brew(ctx, runner, host, auto_install)?;
    check_or_install_lima(ctx, runner, host, &brew_path, auto_install)?;
    check_or_install_socket_vmnet(ctx, runner, host, &brew_path, auto_install)?;

    let mut bin_prefix = runner
        .run_shell(ctx, &format!("{} --prefix", brew_path))
        .unwrap_or_default()
        .trim()
        .to_string();
    if bin_prefix.is_empty() {
        bin_prefix = "/opt/homebrew".to_string();
    }
    let limactl_path = format!("{}/bin/limactl", bin_prefix);

    ensure_sudoers(ctx, runner, host, &limactl_path)?;
    ensure_socket_vmnet_running(ctx, runner, host, &brew_path)?;

    let _ = runner.run_shell(
        ctx,
        &format!(
            "sudo chmod 755 $({} --prefix)/var/run 2>/dev/null",
            brew_path
        ),
    );

    ensure_lima_bridged_network(ctx, runner, host, &bin_prefix)?;

    logging::info("all prerequisites satisfied", &[("host", host)]);
    println!("  [{}] ✅ All prerequisites satisfied", host);
    Ok(())
}

pub fn detect_or_install_brew(
    ctx: &Context,
    runner: &Runner,
    host: &str,
    auto_install: bool,
) -> Result<String, String> {
    if let Ok(out) = runner.run_shell(ctx, BREW_DETECT) {
        let brew_path = out.trim();
        if !brew_path.is_empty() {
            logging::debug("homebrew found", &[("host", host), ("path", brew_path)]);
            return Ok(brew_path.to_string());
        }
    }

    if !auto_install {
        return Err(format!(
            "[{}] homebrew is not installed (use --auto-install to install automatically)",
            host
        ));
    }

    logging::info("installing homebrew", &[("host", host)]);
    println!("  [{}] Installing Homebrew...", host);
    runner
        .run_shell(
            ctx,
            r#"NONINTERACTIVE=1 /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)""#,
        )
        .map_err(|e| format!("[{}] failed to install homebrew: {}", host, e))?;

    let out = runner
        .run_shell(ctx, BREW_DETECT)
        .map_err(|e| format!("[{}] homebrew installed but not detected: {}", host, e))?;
    Ok(out.trim().to_string())
}

pub fn check_or_install_lima(
    ctx: &Context,
    runner: &Runner,
    host: &str,
    brew_path: &str,
    auto_install: bool,
) -> Result<(), String> {
    let mut prefix = runner
        .run_shell(ctx, &format!("{} --prefix", brew_path))
        .unwrap_or_default()
        .trim()
        .to_string();
    if prefix.is_empty() {
        prefix = "/opt/homebrew".to_string();
    }
    let limactl_path = format!("{}/bin/limactl", prefix);

    if let Ok(out) = runner.run_shell(ctx, &format!("{} --version 2>/dev/null", limactl_path)) {
        if !out.trim().is_empty() {
            logging::debug("lima found", &[("host", host), ("version", out.trim())]);
            return Ok(());
        }
    }

    if !auto_install {
        return Err(format!(
            "[{}] lima is not installed (use --auto-install to install automatically)",
            host
        ));
    }

    logging::info("installing lima", &[("host", host)]);
    println!("  [{}] Installing Lima...", host);
    runner
        .run_shell(ctx, &format!("{} install lima", brew_path))
        .map_err(|e| format!("[{}] failed to install lima: {}", host, e))?;
    Ok(())
}

pub fn check_or_install_socket_vmnet(
    ctx: &Context,
    runner: &Runner,
    host: &str,
    brew_path: &str,
    auto_install: bool,
) -> Result<(), String> {
    if runner
        .run_shell(ctx, &format!("{} list socket_vmnet &>/dev/null", brew_path))
        .is_ok()
    {
        logging::debug("socket_vmnet found", &[("host", host)]);
        return Ok(());
    }

    if !auto_install {
        return Err(format!(
            "[{}] socket_vmnet is not installed (use --auto-install to install automatically)",
            host
        ));
    }

    logging::info("installing socket_vmnet", &[("host", host)]);
    println!("  [{}] Installing socket_vmnet...", host);
    runner
        .run_shell(ctx, &format!("{} install socket_vmnet", brew_path))
        .map_err(|e| format!("[{}] failed to install socket_vmnet: {}", host, e))?;
    Ok(())
}

pub fn ensure_sudoers(
    ctx: &Context,
    runner: &Runner,
    host: &str,
    limactl_path: &str,
) -> Result<(), String> {
    if runner
        .run(ctx, "test -f /etc/sudoers.d/lima && echo exists")
        .is_ok()
    {
        logging::debug("lima sudoers already configured", &[("host", host)]);
        return Ok(());
    }

    logging::info("configuring lima sudoers", &[("host", host)]);
    println!("  [{}] Configuring Lima sudoers...", host);
    runner
        .run_shell(
            ctx,
            &format!(
                "{} sudoers | sudo tee /etc/sudoers.d/lima >/dev/null",
                limactl_path
            ),
        )
        .map_err(|e| format!("[{}] failed to configure sudoers: {}", host, e))?;
    Ok(())
}

/// Generates the launchd plist XML for running `socket_vmnet` in bridged mode.
pub fn generate_socket_vmnet_plist(
    socket_bin: &str,
    active_iface: &str,
    socket_path: &str,
    log_dir: &str,
) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>homebrew.mxcl.socket_vmnet</string>
  <key>ProgramArguments</key>
  <array>
    <string>{}</string>
    <string>--vmnet-mode=bridged</string>
    <string>--vmnet-interface={}</string>
    <string>{}</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>StandardErrorPath</key>
  <string>{}/stderr</string>
  <key>StandardOutPath</key>
  <string>{}/stdout</string>
</dict>
</plist>"#,
        socket_bin, active_iface, socket_path, log_dir, log_dir
    )
}

pub fn ensure_socket_vmnet_running(
    ctx: &Context,
    runner: &Runner,
    host: &str,
    brew_path: &str,
) -> Result<(), String> {
    let mut active_iface = runner
        .run_shell(
            ctx,
            "route -n get default 2>/dev/null | grep 'interface:' | sed 's/.*interface: //'",
        )
        .unwrap_or_default()
        .trim()
        .to_string();
    if active_iface.is_empty() {
        active_iface = "en0".to_string();
    }
    logging::debug(
        "detected active network interface",
        &[("host", host), ("interface", &active_iface)],
    );

    let socket_paths = [
        "/opt/homebrew/var/run/socket_vmnet",
        "/usr/local/var/run/socket_vmnet",
        "/var/run/socket_vmnet",
    ];
    for p in socket_paths {
        if runner
            .run_shell(ctx, &format!("sudo test -S {}", p))
            .is_ok()
        {
            let out = runner
                .run_shell(
                    ctx,
                    "cat /Library/LaunchDaemons/homebrew.mxcl.socket_vmnet.plist 2>/dev/null",
                )
                .unwrap_or_default();
            if out.contains("--vmnet-mode=bridged")
                && out.contains(&format!("--vmnet-interface={}", active_iface))
            {
                logging::debug(
                    "socket_vmnet running in bridged mode on correct interface",
                    &[("host", host), ("path", p), ("interface", &active_iface)],
                );
                return Ok(());
            }
            logging::info(
                "reconfiguring socket_vmnet for bridged mode",
                &[("host", host), ("interface", &active_iface)],
            );
            break;
        }
    }

    let mut prefix = runner
        .run_shell(ctx, &format!("{} --prefix", brew_path))
        .unwrap_or_default()
        .trim()
        .to_string();
    if prefix.is_empty() {
        prefix = "/opt/homebrew".to_string();
    }

    let mut socket_bin = runner
        .run_shell(
            ctx,
            &format!(
                "ls {}/Cellar/socket_vmnet/*/bin/socket_vmnet 2>/dev/null | head -1",
                prefix
            ),
        )
        .unwrap_or_default()
        .trim()
        .to_string();
    if socket_bin.is_empty() {
        socket_bin = format!("{}/bin/socket_vmnet", prefix);
    }

    let socket_path = format!("{}/var/run/socket_vmnet", prefix);
    let log_dir = format!("{}/var/log/socket_vmnet", prefix);

    logging::info(
        "configuring socket_vmnet in bridged mode",
        &[("host", host), ("interface", &active_iface)],
    );
    println!(
        "  [{}] Configuring socket_vmnet bridged mode (interface={})...",
        host, active_iface
    );

    let _ = runner.run_shell(
        ctx,
        &format!("sudo {} services stop socket_vmnet 2>/dev/null", brew_path),
    );
    let _ = runner.run_shell(
        ctx,
        "sudo launchctl bootout system /Library/LaunchDaemons/homebrew.mxcl.socket_vmnet.plist 2>/dev/null",
    );
    let _ = runner.run_shell(
        ctx,
        "sudo launchctl bootout system/homebrew.mxcl.socket_vmnet 2>/dev/null",
    );
    let _ = runner.run_shell(ctx, "sleep 1");

    let plist = generate_socket_vmnet_plist(&socket_bin, &active_iface, &socket_path, &log_dir);
    let encoded = base64_encode(plist.as_bytes());
    runner
        .run_shell(
            ctx,
            &format!(
                "sudo mkdir -p {} && echo {} | base64 -d | sudo tee /Library/LaunchDaemons/homebrew.mxcl.socket_vmnet.plist > /dev/null",
                log_dir, encoded
            ),
        )
        .map_err(|e| format!("[{}] failed to write socket_vmnet plist: {}", host, e))?;

    let _ = runner.run_shell(
        ctx,
        &format!(
            "sudo mkdir -p $(dirname {}) && sudo chmod 755 $(dirname {})",
            socket_path, socket_path
        ),
    );

    let _ = runner.run_shell(
        ctx,
        "sudo launchctl enable system/homebrew.mxcl.socket_vmnet 2>/dev/null",
    );
    runner
        .run_shell(
            ctx,
            "sudo launchctl bootstrap system /Library/LaunchDaemons/homebrew.mxcl.socket_vmnet.plist",
        )
        .map_err(|e| format!("[{}] failed to start socket_vmnet bridged service: {}", host, e))?;

    let _ = runner.run_shell(ctx, "sleep 2");
    runner
        .run_shell(ctx, &format!("sudo test -S {}", socket_path))
        .map_err(|_| {
            format!(
                "[{}] socket_vmnet started but socket not found at {}",
                host, socket_path
            )
        })?;

    logging::info(
        "socket_vmnet running in bridged mode",
        &[("host", host), ("interface", &active_iface)],
    );
    Ok(())
}

pub fn ensure_lima_bridged_network(
    _ctx: &Context,
    _runner: &Runner,
    _host: &str,
    _bin_prefix: &str,
) -> Result<(), String> {
    Ok(())
}

fn new_runner(node: &NodeConfig) -> Runner {
    if !node.ssh_user.is_empty() || !node.ssh_port.is_empty() || !node.ssh_key_path.is_empty() {
        Runner::with_opts(
            &node.host,
            &node.ssh_user,
            &node.ssh_port,
            &node.ssh_key_path,
        )
    } else {
        Runner::new(&node.host)
    }
}
