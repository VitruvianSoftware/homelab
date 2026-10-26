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

//! Package cmd implements the top-level CLI command and subcommands (`init`,
//! `apply`, `join`, `remove`, `destroy`, `upgrade`, `backup`, `restore`,
//! `doctor`, `status`, `version`).

use std::time::Duration;

use crate::cluster::{self, InitOptions};
use crate::config::Config;
use crate::doctor;
use crate::logging;
use crate::remote::Context;

/// BuildInfo holds compile-time/runtime version metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildInfo {
    pub version: String,
    pub commit: String,
    pub date: String,
}

impl Default for BuildInfo {
    fn default() -> Self {
        Self {
            version: "dev".to_string(),
            commit: "none".to_string(),
            date: "unknown".to_string(),
        }
    }
}

/// Parsed CLI invocation representing global flags and subcommand arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedCli {
    pub config_file: String,
    pub verbose: bool,
    pub command: Subcommand,
}

/// Supported CLI subcommands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Subcommand {
    Help(Option<String>),
    Version,
    Init {
        dry_run: bool,
        auto_install: bool,
        timeout: Duration,
    },
    Apply {
        dry_run: bool,
    },
    Join {
        dry_run: bool,
        timeout: Duration,
    },
    Remove {
        node_host: String,
        dry_run: bool,
        timeout: Duration,
    },
    Destroy {
        force: bool,
        dry_run: bool,
        timeout: Duration,
    },
    Upgrade {
        dry_run: bool,
        timeout: Duration,
    },
    Backup {
        output_dir: String,
        timeout: Duration,
    },
    Restore {
        snapshot_path: String,
        timeout: Duration,
    },
    Doctor,
    Status,
}

/// RootCmd is the top-level CLI dispatcher.
#[derive(Debug, Clone)]
pub struct RootCmd {
    pub info: BuildInfo,
}

impl RootCmd {
    /// Creates a new `RootCmd` with the given version metadata.
    pub fn new(version: &str, commit: &str, date: &str) -> Self {
        Self {
            info: BuildInfo {
                version: version.to_string(),
                commit: commit.to_string(),
                date: date.to_string(),
            },
        }
    }

    /// Formats the version output string (`homelab <version> (commit: <commit>, built: <date>)`).
    pub fn format_version(&self) -> String {
        format!(
            "homelab {} (commit: {}, built: {})",
            self.info.version, self.info.commit, self.info.date
        )
    }

    /// Formats the root help text matching the Cobra CLI layout.
    pub fn format_help() -> String {
        r#"homelab is a CLI tool for provisioning and managing Kubernetes clusters
across multiple macOS machines using Lima VZ and K3s.

Define your cluster topology in a YAML config file, then use homelab to
bootstrap, scale, diagnose, and tear down your cluster.

Usage:
  homelab [command]

Available Commands:
  apply       Apply configuration changes iteratively with zero downtime
  backup      Create an etcd snapshot backup
  destroy     Tear down the entire cluster
  doctor      Run pre-flight checks and health diagnostics
  init        Bootstrap a new cluster from the config file
  join        Add new node(s) to an existing cluster
  remove      Drain and remove a node from the cluster
  restore     Restore from an etcd snapshot
  status      Show the current cluster state
  upgrade     Rolling upgrade K3s to the version specified in config
  version     Print the version information

Flags:
  -c, --config string   path to cluster config file (default "homelab.yaml")
  -h, --help            help for homelab
  -v, --verbose         enable debug logging"#
            .to_string()
    }

    /// Parses command-line arguments (excluding `argv[0]`).
    pub fn parse_args(&self, args: &[String]) -> Result<ParsedCli, String> {
        let mut config_file = "homelab.yaml".to_string();
        let mut verbose = false;
        let mut dry_run = false;
        let mut auto_install = false;
        let mut force = false;
        let mut timeout: Option<Duration> = None;
        let mut output_dir = ".".to_string();
        let mut snapshot_path = String::new();
        let mut help_requested = false;
        let mut version_flag = false;

        let mut positionals: Vec<String> = Vec::new();
        let mut i = 0;

        while i < args.len() {
            let arg = &args[i];
            if arg == "--" {
                positionals.extend_from_slice(&args[i + 1..]);
                break;
            } else if arg == "-h" || arg == "--help" {
                help_requested = true;
            } else if arg == "--version" {
                version_flag = true;
            } else if arg == "-v" || arg == "--verbose" {
                verbose = true;
            } else if let Some(val) = arg.strip_prefix("--verbose=") {
                verbose = parse_bool_flag("verbose", val)?;
            } else if arg == "-c" || arg == "--config" {
                i += 1;
                if i >= args.len() {
                    return Err("flag needs an argument: --config".to_string());
                }
                config_file = args[i].clone();
            } else if let Some(val) = arg.strip_prefix("--config=") {
                config_file = val.to_string();
            } else if arg == "--dry-run" {
                dry_run = true;
            } else if let Some(val) = arg.strip_prefix("--dry-run=") {
                dry_run = parse_bool_flag("dry-run", val)?;
            } else if arg == "--auto-install" {
                auto_install = true;
            } else if let Some(val) = arg.strip_prefix("--auto-install=") {
                auto_install = parse_bool_flag("auto-install", val)?;
            } else if arg == "--force" {
                force = true;
            } else if let Some(val) = arg.strip_prefix("--force=") {
                force = parse_bool_flag("force", val)?;
            } else if arg == "--timeout" {
                i += 1;
                if i >= args.len() {
                    return Err("flag needs an argument: --timeout".to_string());
                }
                timeout = Some(parse_duration(&args[i])?);
            } else if let Some(val) = arg.strip_prefix("--timeout=") {
                timeout = Some(parse_duration(val)?);
            } else if arg == "-o" || arg == "--output" {
                i += 1;
                if i >= args.len() {
                    return Err("flag needs an argument: --output".to_string());
                }
                output_dir = args[i].clone();
            } else if let Some(val) = arg.strip_prefix("--output=") {
                output_dir = val.to_string();
            } else if arg == "-s" || arg == "--snapshot" {
                i += 1;
                if i >= args.len() {
                    return Err("flag needs an argument: --snapshot".to_string());
                }
                snapshot_path = args[i].clone();
            } else if let Some(val) = arg.strip_prefix("--snapshot=") {
                snapshot_path = val.to_string();
            } else if arg.starts_with('-') {
                return Err(format!("unknown flag: {}", arg));
            } else {
                positionals.push(arg.clone());
            }
            i += 1;
        }

        if version_flag {
            return Ok(ParsedCli {
                config_file,
                verbose,
                command: Subcommand::Version,
            });
        }

        if help_requested || positionals.is_empty() {
            return Ok(ParsedCli {
                config_file,
                verbose,
                command: Subcommand::Help(positionals.first().cloned()),
            });
        }

        let sub = positionals[0].as_str();
        let sub_args = &positionals[1..];

        let command = match sub {
            "help" => Subcommand::Help(sub_args.first().cloned()),
            "version" => Subcommand::Version,
            "init" => Subcommand::Init {
                dry_run,
                auto_install,
                timeout: timeout.unwrap_or_else(|| Duration::from_secs(30 * 60)),
            },
            "apply" => Subcommand::Apply { dry_run },
            "join" => Subcommand::Join {
                dry_run,
                timeout: timeout.unwrap_or_else(|| Duration::from_secs(10 * 60)),
            },
            "remove" => {
                if sub_args.len() != 1 {
                    return Err(format!("accepts 1 arg(s), received {}", sub_args.len()));
                }
                Subcommand::Remove {
                    node_host: sub_args[0].clone(),
                    dry_run,
                    timeout: timeout.unwrap_or_else(|| Duration::from_secs(10 * 60)),
                }
            }
            "destroy" => Subcommand::Destroy {
                force,
                dry_run,
                timeout: timeout.unwrap_or_else(|| Duration::from_secs(15 * 60)),
            },
            "upgrade" => Subcommand::Upgrade {
                dry_run,
                timeout: timeout.unwrap_or_else(|| Duration::from_secs(45 * 60)),
            },
            "backup" => Subcommand::Backup {
                output_dir,
                timeout: timeout.unwrap_or_else(|| Duration::from_secs(10 * 60)),
            },
            "restore" => {
                if snapshot_path.is_empty() {
                    return Err("required flag(s) \"snapshot\" not set".to_string());
                }
                Subcommand::Restore {
                    snapshot_path,
                    timeout: timeout.unwrap_or_else(|| Duration::from_secs(15 * 60)),
                }
            }
            "doctor" => Subcommand::Doctor,
            "status" => Subcommand::Status,
            other => return Err(format!("unknown command {:?} for \"homelab\"", other)),
        };

        Ok(ParsedCli {
            config_file,
            verbose,
            command,
        })
    }

    /// Executes the CLI with the provided arguments (excluding `argv[0]`).
    pub fn execute_with_args(&self, args: &[String]) -> Result<(), String> {
        let parsed = self.parse_args(args)?;
        logging::setup(parsed.verbose);

        match parsed.command {
            Subcommand::Help(_) => {
                println!("{}", Self::format_help());
                Ok(())
            }
            Subcommand::Version => {
                println!("{}", self.format_version());
                Ok(())
            }
            Subcommand::Init {
                dry_run,
                auto_install,
                timeout,
            } => {
                let cfg = Config::load(&parsed.config_file)
                    .map_err(|e| format!("loading config: {}", e))?;
                let (ctx, _) = Context::with_timeout(timeout);
                cluster::init(
                    &ctx,
                    &cfg,
                    InitOptions {
                        dry_run,
                        auto_install,
                    },
                )
            }
            Subcommand::Apply { dry_run } => {
                let cfg = Config::load(&parsed.config_file)
                    .map_err(|e| format!("loading config: {}", e))?;
                let ctx = Context::background();
                cluster::apply(&ctx, &cfg, dry_run)
            }
            Subcommand::Join { dry_run, timeout } => {
                let cfg = Config::load(&parsed.config_file)
                    .map_err(|e| format!("loading config: {}", e))?;
                let (ctx, _) = Context::with_timeout(timeout);
                cluster::join(&ctx, &cfg, dry_run)
            }
            Subcommand::Remove {
                node_host,
                dry_run,
                timeout,
            } => {
                let cfg = Config::load(&parsed.config_file)
                    .map_err(|e| format!("loading config: {}", e))?;
                let (ctx, _) = Context::with_timeout(timeout);
                cluster::remove(&ctx, &cfg, &node_host, dry_run)
            }
            Subcommand::Destroy {
                force,
                dry_run,
                timeout,
            } => {
                let cfg = Config::load(&parsed.config_file)
                    .map_err(|e| format!("loading config: {}", e))?;
                let (ctx, _) = Context::with_timeout(timeout);
                cluster::destroy(&ctx, &cfg, force, dry_run)
            }
            Subcommand::Upgrade { dry_run, timeout } => {
                let cfg = Config::load(&parsed.config_file)
                    .map_err(|e| format!("loading config: {}", e))?;
                let (ctx, _) = Context::with_timeout(timeout);
                cluster::upgrade(&ctx, &cfg, dry_run)
            }
            Subcommand::Backup {
                output_dir,
                timeout,
            } => {
                let cfg = Config::load(&parsed.config_file)
                    .map_err(|e| format!("loading config: {}", e))?;
                let (ctx, _) = Context::with_timeout(timeout);
                cluster::backup(&ctx, &cfg, &output_dir)
            }
            Subcommand::Restore {
                snapshot_path,
                timeout,
            } => {
                let cfg = Config::load(&parsed.config_file)
                    .map_err(|e| format!("loading config: {}", e))?;
                let (ctx, _) = Context::with_timeout(timeout);
                cluster::restore(&ctx, &cfg, &snapshot_path)
            }
            Subcommand::Doctor => {
                let cfg = Config::load(&parsed.config_file)
                    .map_err(|e| format!("loading config: {}", e))?;
                let ctx = Context::background();
                doctor::run(&ctx, &cfg)
            }
            Subcommand::Status => {
                let cfg = Config::load(&parsed.config_file)
                    .map_err(|e| format!("loading config: {}", e))?;
                let ctx = Context::background();
                cluster::status(&ctx, &cfg)
            }
        }
    }

    /// Executes the CLI reading arguments from `std::env::args()`.
    pub fn execute(&self) -> Result<(), String> {
        let args: Vec<String> = std::env::args().skip(1).collect();
        self.execute_with_args(&args)
    }
}

fn parse_bool_flag(flag: &str, val: &str) -> Result<bool, String> {
    match val {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        _ => Err(format!("invalid boolean value {:?} for --{}", val, flag)),
    }
}

/// Parses Go-style duration strings such as `"30m"`, `"45m0s"`, `"10s"`, `"1h"`, `"500ms"`.
pub fn parse_duration(s: &str) -> Result<Duration, String> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Err("empty duration string".to_string());
    }

    let mut total = Duration::ZERO;
    let mut rest = trimmed;

    while !rest.is_empty() {
        let num_end = rest
            .find(|c: char| !c.is_ascii_digit())
            .ok_or_else(|| format!("missing unit in duration {:?}", s))?;
        if num_end == 0 {
            return Err(format!("invalid duration {:?}", s));
        }
        let val: u64 = rest[..num_end]
            .parse()
            .map_err(|_| format!("invalid duration number in {:?}", s))?;
        let after_num = &rest[num_end..];

        let unit_end = after_num
            .find(|c: char| c.is_ascii_digit())
            .unwrap_or(after_num.len());
        let unit = &after_num[..unit_end];

        let part = match unit {
            "ms" => Duration::from_millis(val),
            "s" => Duration::from_secs(val),
            "m" => Duration::from_secs(val * 60),
            "h" => Duration::from_secs(val * 3600),
            _ => return Err(format!("unknown unit {:?} in duration {:?}", unit, s)),
        };

        total += part;
        rest = &after_num[unit_end..];
    }

    Ok(total)
}
