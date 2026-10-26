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

//! Native Rust implementation of the `homelab` k3s cluster operations CLI.

pub mod cluster;
pub mod cmd;
pub mod config;
pub mod doctor;
pub mod k3s;
pub mod lima;
pub mod logging;
pub mod prereqs;
pub mod remote;

#[cfg(test)]
mod tests {
    use std::env;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use crate::cluster::{self, InitOptions};
    use crate::cmd::{RootCmd, Subcommand};
    use crate::config::{ClusterConfig, Config, NodeConfig, VMConfig};
    use crate::doctor;
    use crate::k3s;
    use crate::lima::{self, VMStatus};
    use crate::logging;
    use crate::prereqs;
    use crate::remote::{is_transient, Context, Runner, DEFAULT_MAX_RETRIES};

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn write_temp(content: &str) -> PathBuf {
        let idx = TEMP_COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = env::temp_dir().join(format!("homelab_rs_test_{}_{}", std::process::id(), idx));
        fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("config.yaml");
        fs::write(&path, content.as_bytes()).expect("write temp config");
        path
    }

    fn resolve_data_file(rel: &str) -> PathBuf {
        let candidates = [
            PathBuf::from(format!("apps/cli/homelab/{}", rel)),
            PathBuf::from(rel),
        ];
        for c in &candidates {
            if c.exists() {
                return c.clone();
            }
        }
        if let Ok(srcdir) = env::var("TEST_SRCDIR") {
            let ws = env::var("TEST_WORKSPACE").unwrap_or_else(|_| "_main".to_string());
            let p = PathBuf::from(srcdir)
                .join(ws)
                .join("apps/cli/homelab")
                .join(rel);
            if p.exists() {
                return p;
            }
        }
        panic!("could not locate test data file: {}", rel);
    }

    // =========================================================================
    // Ported from internal/config/config_test.go
    // =========================================================================

    #[test]
    fn test_load_valid_config() {
        let content = r#"
cluster:
  name: test-cluster
  k3sVersion: "v1.31.2+k3s1"
  kubeconfig: "~/.kube/test.yaml"
nodes:
  - host: host-1
    role: server
    pool: cp-1
    vm:
      cpus: 2
      memory: 4GiB
      disk: 30GiB
  - host: host-2
    role: server
    pool: cp-2
    vm:
      cpus: 2
      memory: 4GiB
      disk: 30GiB
  - host: host-3
    role: server
    pool: cp-3
    vm:
      cpus: 2
      memory: 4GiB
      disk: 30GiB
  - host: host-4
    role: agent
    pool: worker-1
    vm:
      cpus: 4
      memory: 8GiB
      disk: 50GiB
"#;
        let path = write_temp(content);
        let cfg = Config::load(path.to_str().unwrap()).expect("unexpected error");
        assert_eq!(cfg.cluster.name, "test-cluster");
        assert_eq!(cfg.cluster.k3s_version, "v1.31.2+k3s1");
        assert_eq!(cfg.cluster.kubeconfig, "~/.kube/test.yaml");
        assert_eq!(cfg.nodes.len(), 4);
    }

    #[test]
    fn test_load_file_not_found() {
        let res = Config::load("/nonexistent/path/config.yaml");
        assert!(res.is_err(), "expected error for missing file");
    }

    #[test]
    fn test_load_malformed_yaml() {
        let path = write_temp("{{invalid yaml}}");
        let res = Config::load(path.to_str().unwrap());
        assert!(res.is_err(), "expected error for malformed YAML");
    }

    #[test]
    fn test_load_env_var() {
        let content = r#"
cluster:
  name: env-test
nodes:
  - host: host-1
    role: server
    pool: cp-1
    vm:
      cpus: 1
      memory: 1GiB
      disk: 10GiB
"#;
        let path = write_temp(content);
        env::set_var("HOMELAB_CONFIG", path.to_str().unwrap());
        let cfg = Config::load("homelab.yaml").expect("unexpected error");
        env::remove_var("HOMELAB_CONFIG");
        assert_eq!(cfg.cluster.name, "env-test");
    }

    #[test]
    fn test_server_nodes() {
        let cfg = Config {
            cluster: ClusterConfig::default(),
            nodes: vec![
                NodeConfig {
                    host: "a".into(),
                    role: "server".into(),
                    pool: "p1".into(),
                    vm: VMConfig {
                        cpus: 1,
                        memory: "1GiB".into(),
                        disk: "10GiB".into(),
                    },
                    ..NodeConfig::default()
                },
                NodeConfig {
                    host: "b".into(),
                    role: "agent".into(),
                    pool: "p2".into(),
                    vm: VMConfig {
                        cpus: 1,
                        memory: "1GiB".into(),
                        disk: "10GiB".into(),
                    },
                    ..NodeConfig::default()
                },
                NodeConfig {
                    host: "c".into(),
                    role: "server".into(),
                    pool: "p3".into(),
                    vm: VMConfig {
                        cpus: 1,
                        memory: "1GiB".into(),
                        disk: "10GiB".into(),
                    },
                    ..NodeConfig::default()
                },
            ],
        };
        let servers = cfg.server_nodes();
        assert_eq!(servers.len(), 2);
    }

    #[test]
    fn test_agent_nodes() {
        let cfg = Config {
            cluster: ClusterConfig::default(),
            nodes: vec![
                NodeConfig {
                    host: "a".into(),
                    role: "server".into(),
                    pool: "p1".into(),
                    vm: VMConfig {
                        cpus: 1,
                        memory: "1GiB".into(),
                        disk: "10GiB".into(),
                    },
                    ..NodeConfig::default()
                },
                NodeConfig {
                    host: "b".into(),
                    role: "agent".into(),
                    pool: "p2".into(),
                    vm: VMConfig {
                        cpus: 1,
                        memory: "1GiB".into(),
                        disk: "10GiB".into(),
                    },
                    ..NodeConfig::default()
                },
                NodeConfig {
                    host: "c".into(),
                    role: "agent".into(),
                    pool: "p3".into(),
                    vm: VMConfig {
                        cpus: 1,
                        memory: "1GiB".into(),
                        disk: "10GiB".into(),
                    },
                    ..NodeConfig::default()
                },
            ],
        };
        let agents = cfg.agent_nodes();
        assert_eq!(agents.len(), 2);
    }

    #[test]
    fn test_init_node() {
        let cfg = Config {
            cluster: ClusterConfig::default(),
            nodes: vec![
                NodeConfig {
                    host: "first-server".into(),
                    role: "server".into(),
                    pool: "p1".into(),
                    vm: VMConfig {
                        cpus: 1,
                        memory: "1GiB".into(),
                        disk: "10GiB".into(),
                    },
                    ..NodeConfig::default()
                },
                NodeConfig {
                    host: "second-server".into(),
                    role: "server".into(),
                    pool: "p2".into(),
                    vm: VMConfig {
                        cpus: 1,
                        memory: "1GiB".into(),
                        disk: "10GiB".into(),
                    },
                    ..NodeConfig::default()
                },
            ],
        };
        let init = cfg.init_node();
        assert_eq!(init.host, "first-server");
    }

    #[test]
    fn test_get_vm_name_default() {
        let n = NodeConfig {
            host: "test".into(),
            ..NodeConfig::default()
        };
        assert_eq!(n.get_vm_name(), "k8s-node");
    }

    #[test]
    fn test_get_vm_name_custom() {
        let n = NodeConfig {
            host: "test".into(),
            vm_name: "custom-vm".into(),
            ..NodeConfig::default()
        };
        assert_eq!(n.get_vm_name(), "custom-vm");
    }

    // =========================================================================
    // Ported from internal/config/validate_test.go
    // =========================================================================

    fn sample_vm(cpus: i32, memory: &str, disk: &str) -> VMConfig {
        VMConfig {
            cpus,
            memory: memory.into(),
            disk: disk.into(),
        }
    }

    #[test]
    fn test_validate_valid() {
        let cfg = Config {
            cluster: ClusterConfig {
                name: "test".into(),
                ..ClusterConfig::default()
            },
            nodes: vec![
                NodeConfig {
                    host: "a".into(),
                    role: "server".into(),
                    pool: "p1".into(),
                    vm: sample_vm(2, "4GiB", "30GiB"),
                    ..NodeConfig::default()
                },
                NodeConfig {
                    host: "b".into(),
                    role: "server".into(),
                    pool: "p2".into(),
                    vm: sample_vm(2, "4GiB", "30GiB"),
                    ..NodeConfig::default()
                },
                NodeConfig {
                    host: "c".into(),
                    role: "server".into(),
                    pool: "p3".into(),
                    vm: sample_vm(2, "4GiB", "30GiB"),
                    ..NodeConfig::default()
                },
                NodeConfig {
                    host: "d".into(),
                    role: "agent".into(),
                    pool: "p4".into(),
                    vm: sample_vm(4, "8GiB", "50GiB"),
                    ..NodeConfig::default()
                },
            ],
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn test_validate_missing_cluster_name() {
        let cfg = Config {
            cluster: ClusterConfig::default(),
            nodes: vec![NodeConfig {
                host: "a".into(),
                role: "server".into(),
                pool: "p1".into(),
                vm: sample_vm(1, "1GiB", "10GiB"),
                ..NodeConfig::default()
            }],
        };
        let err = cfg
            .validate()
            .expect_err("expected error for missing cluster name");
        assert!(err.contains("cluster.name is required"));
    }

    #[test]
    fn test_validate_no_nodes() {
        let cfg = Config {
            cluster: ClusterConfig {
                name: "test".into(),
                ..ClusterConfig::default()
            },
            nodes: vec![],
        };
        let err = cfg.validate().expect_err("expected error for no nodes");
        assert!(err.contains("at least one node must be defined"));
    }

    #[test]
    fn test_validate_no_server_nodes() {
        let cfg = Config {
            cluster: ClusterConfig {
                name: "test".into(),
                ..ClusterConfig::default()
            },
            nodes: vec![NodeConfig {
                host: "a".into(),
                role: "agent".into(),
                pool: "p1".into(),
                vm: sample_vm(1, "1GiB", "10GiB"),
                ..NodeConfig::default()
            }],
        };
        let err = cfg
            .validate()
            .expect_err("expected error for no server nodes");
        assert!(err.contains("at least one server node is required"));
    }

    #[test]
    fn test_validate_even_server_count() {
        let cfg = Config {
            cluster: ClusterConfig {
                name: "test".into(),
                ..ClusterConfig::default()
            },
            nodes: vec![
                NodeConfig {
                    host: "a".into(),
                    role: "server".into(),
                    pool: "p1".into(),
                    vm: sample_vm(1, "1GiB", "10GiB"),
                    ..NodeConfig::default()
                },
                NodeConfig {
                    host: "b".into(),
                    role: "server".into(),
                    pool: "p2".into(),
                    vm: sample_vm(1, "1GiB", "10GiB"),
                    ..NodeConfig::default()
                },
            ],
        };
        let err = cfg
            .validate()
            .expect_err("expected error for even server count");
        assert!(err.contains("HA requires an odd number of server nodes for etcd quorum, got 2"));
    }

    #[test]
    fn test_validate_single_server() {
        let cfg = Config {
            cluster: ClusterConfig {
                name: "test".into(),
                ..ClusterConfig::default()
            },
            nodes: vec![NodeConfig {
                host: "a".into(),
                role: "server".into(),
                pool: "p1".into(),
                vm: sample_vm(1, "1GiB", "10GiB"),
                ..NodeConfig::default()
            }],
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn test_validate_invalid_role() {
        let cfg = Config {
            cluster: ClusterConfig {
                name: "test".into(),
                ..ClusterConfig::default()
            },
            nodes: vec![NodeConfig {
                host: "a".into(),
                role: "master".into(),
                pool: "p1".into(),
                vm: sample_vm(1, "1GiB", "10GiB"),
                ..NodeConfig::default()
            }],
        };
        let err = cfg.validate().expect_err("expected error for invalid role");
        assert!(err.contains("nodes[0].role must be 'server' or 'agent', got \"master\""));
    }

    #[test]
    fn test_validate_duplicate_hosts() {
        let cfg = Config {
            cluster: ClusterConfig {
                name: "test".into(),
                ..ClusterConfig::default()
            },
            nodes: vec![
                NodeConfig {
                    host: "same-host".into(),
                    role: "server".into(),
                    pool: "p1".into(),
                    vm: sample_vm(1, "1GiB", "10GiB"),
                    ..NodeConfig::default()
                },
                NodeConfig {
                    host: "same-host".into(),
                    role: "agent".into(),
                    pool: "p2".into(),
                    vm: sample_vm(1, "1GiB", "10GiB"),
                    ..NodeConfig::default()
                },
            ],
        };
        let err = cfg
            .validate()
            .expect_err("expected error for duplicate hosts");
        assert!(err.contains("duplicate host: \"same-host\""));
    }

    #[test]
    fn test_validate_duplicate_pools() {
        let cfg = Config {
            cluster: ClusterConfig {
                name: "test".into(),
                ..ClusterConfig::default()
            },
            nodes: vec![
                NodeConfig {
                    host: "a".into(),
                    role: "server".into(),
                    pool: "same-pool".into(),
                    vm: sample_vm(1, "1GiB", "10GiB"),
                    ..NodeConfig::default()
                },
                NodeConfig {
                    host: "b".into(),
                    role: "agent".into(),
                    pool: "same-pool".into(),
                    vm: sample_vm(1, "1GiB", "10GiB"),
                    ..NodeConfig::default()
                },
            ],
        };
        let err = cfg
            .validate()
            .expect_err("expected error for duplicate pools");
        assert!(err.contains("duplicate pool label: \"same-pool\""));
    }

    #[test]
    fn test_validate_missing_host() {
        let cfg = Config {
            cluster: ClusterConfig {
                name: "test".into(),
                ..ClusterConfig::default()
            },
            nodes: vec![NodeConfig {
                role: "server".into(),
                pool: "p1".into(),
                vm: sample_vm(1, "1GiB", "10GiB"),
                ..NodeConfig::default()
            }],
        };
        let err = cfg.validate().expect_err("expected error for missing host");
        assert!(err.contains("nodes[0].host is required"));
    }

    #[test]
    fn test_validate_missing_pool() {
        let cfg = Config {
            cluster: ClusterConfig {
                name: "test".into(),
                ..ClusterConfig::default()
            },
            nodes: vec![NodeConfig {
                host: "a".into(),
                role: "server".into(),
                vm: sample_vm(1, "1GiB", "10GiB"),
                ..NodeConfig::default()
            }],
        };
        let err = cfg.validate().expect_err("expected error for missing pool");
        assert!(err.contains("nodes[0].pool is required"));
    }

    #[test]
    fn test_validate_missing_vm_memory() {
        let cfg = Config {
            cluster: ClusterConfig {
                name: "test".into(),
                ..ClusterConfig::default()
            },
            nodes: vec![NodeConfig {
                host: "a".into(),
                role: "server".into(),
                pool: "p1".into(),
                vm: sample_vm(1, "", "10GiB"),
                ..NodeConfig::default()
            }],
        };
        let err = cfg
            .validate()
            .expect_err("expected error for missing VM memory");
        assert!(err.contains("nodes[0].vm.memory is required"));
    }

    #[test]
    fn test_validate_zero_cpus() {
        let cfg = Config {
            cluster: ClusterConfig {
                name: "test".into(),
                ..ClusterConfig::default()
            },
            nodes: vec![NodeConfig {
                host: "a".into(),
                role: "server".into(),
                pool: "p1".into(),
                vm: sample_vm(0, "1GiB", "10GiB"),
                ..NodeConfig::default()
            }],
        };
        let err = cfg.validate().expect_err("expected error for zero CPUs");
        assert!(err.contains("nodes[0].vm.cpus must be >= 1"));
    }

    // =========================================================================
    // Ported from internal/remote/remote_test.go + SSH/Lima shell tests
    // =========================================================================

    #[test]
    fn test_is_transient() {
        let cases = [
            ("Connection refused", true),
            ("Connection reset by peer", true),
            ("Connection timed out", true),
            ("No route to host", true),
            ("Host is down", true),
            ("Network is unreachable", true),
            (
                "ssh_exchange_identification: read: Connection reset by peer",
                true,
            ),
            ("kex_exchange_identification: write: Broken pipe", true),
            ("Permission denied (publickey)", false),
            ("command not found: kubectl", false),
            ("exit status 1", false),
        ];

        for (msg, want) in cases {
            assert_eq!(
                is_transient(msg),
                want,
                "is_transient({:?}) != {}",
                msg,
                want
            );
        }
    }

    #[test]
    fn test_new_runner() {
        let r = Runner::new("test-host");
        assert_eq!(r.host, "test-host");
        assert_eq!(r.max_retries, DEFAULT_MAX_RETRIES);
    }

    #[test]
    fn test_new_runner_with_opts() {
        let r = Runner::with_opts("host", "user", "2222", "/path/to/key");
        assert_eq!(r.host, "host");
        assert_eq!(r.user, "user");
        assert_eq!(r.port, "2222");
        assert_eq!(r.key_path, "/path/to/key");
    }

    #[test]
    fn test_run_cancelled_context() {
        let mut r = Runner::new("nonexistent-host");
        r.max_retries = 0;

        let (ctx, cancel) = Context::with_cancel();
        cancel.cancel();

        let err = r
            .run(&ctx, "echo test")
            .expect_err("expected error for cancelled context");
        assert!(
            err.contains("context canceled"),
            "expected context canceled error, got: {}",
            err
        );
    }

    #[test]
    fn test_runner_build_ssh_args_and_transient_retry() {
        let r = Runner::with_opts("mac-1", "james", "2222", "/Users/james/.ssh/id_ed25519");
        let args = r.build_ssh_args("limactl list --json");
        assert_eq!(
            args,
            vec![
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=10",
                "-o",
                "StrictHostKeyChecking=accept-new",
                "-p",
                "2222",
                "-i",
                "/Users/james/.ssh/id_ed25519",
                "james@mac-1",
                "export PATH=/opt/homebrew/bin:/usr/local/bin:$PATH; limactl list --json",
            ]
        );

        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_clone = Arc::clone(&attempts);
        let mut retry_runner = Runner::new("mac-1").with_executor(move |_, _| {
            let n = attempts_clone.fetch_add(1, Ordering::SeqCst);
            if n < 2 {
                Err("Connection reset by peer".to_string())
            } else {
                Ok("ok-after-retry\n".to_string())
            }
        });
        retry_runner.backoff_base = Duration::from_millis(1);

        let ctx = Context::background();
        let out = retry_runner
            .run(&ctx, "echo ok")
            .expect("should succeed on 3rd try");
        assert_eq!(out, "ok-after-retry");
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    // =========================================================================
    // Lima, K3s, Prereqs, Doctor, Cluster & CLI integration tests
    // =========================================================================

    #[test]
    fn test_lima_generate_config_and_status() {
        let node = NodeConfig {
            host: "mac-1".into(),
            role: "server".into(),
            pool: "cp-1".into(),
            vm: sample_vm(4, "8GiB", "50GiB"),
            ..NodeConfig::default()
        };

        let runner = Runner::new("mac-1").with_executor(|_, cmd| {
            if cmd == "limactl list --json" {
                Ok(r#"{"name":"other","status":"Stopped"}
{"name":"k8s-node","status":"Running"}"#
                    .to_string())
            } else {
                Ok(String::new())
            }
        });

        let mgr = lima::Manager::new(runner, node);
        let ctx = Context::background();
        assert_eq!(mgr.status(&ctx).unwrap(), VMStatus::Running);

        let cfg_yaml = mgr.generate_config("/opt/homebrew/var/run/socket_vmnet");
        assert!(cfg_yaml.contains("vmType: \"vz\""));
        assert!(cfg_yaml.contains("cpus: 4"));
        assert!(cfg_yaml.contains("memory: \"8GiB\""));
        assert!(cfg_yaml.contains("disk: \"50GiB\""));
        assert!(cfg_yaml.contains("- socket: \"/opt/homebrew/var/run/socket_vmnet\""));
    }

    #[test]
    fn test_k3s_command_builders_and_kubeconfig() {
        let runner = Runner::new("cp-1").with_executor(|_, cmd| {
            if cmd.contains("cat /etc/rancher/k3s/k3s.yaml") {
                Ok("server: https://127.0.0.1:6443\n".to_string())
            } else if cmd.contains("k3s --version") {
                Ok("k3s version v1.31.2+k3s1 (abc1234)".to_string())
            } else {
                Ok(String::new())
            }
        });
        let mgr = k3s::Manager::new(runner);
        let sans = vec!["192.168.1.10".to_string(), "192.168.1.11".to_string()];

        let init_script =
            mgr.build_init_cluster_script("192.168.1.10", "laptop-cp-1", "v1.31.2+k3s1", &sans);
        assert_eq!(
            init_script,
            r#"curl -sfL https://get.k3s.io | INSTALL_K3S_VERSION="v1.31.2+k3s1" INSTALL_K3S_EXEC="server" sh -s - --cluster-init --node-name=cp-1 --node-ip=192.168.1.10 --advertise-address=192.168.1.10 --tls-san=192.168.1.10 --tls-san=192.168.1.11 --flannel-iface=lima0 --node-label=pool=laptop-cp-1"#
        );

        let join_server_script = mgr.build_join_server_script(
            "192.168.1.11",
            "https://192.168.1.10:6443",
            "secret-token",
            "laptop-cp-2",
            "v1.31.2+k3s1",
            &["192.168.1.11".to_string()],
        );
        assert!(join_server_script.contains("INSTALL_K3S_SKIP_START=true"));
        assert!(join_server_script.contains("K3S_TOKEN=\"secret-token\""));
        assert!(join_server_script.contains("--server=https://192.168.1.10:6443"));

        let join_agent_script = mgr.build_join_agent_script(
            "192.168.1.12",
            "https://192.168.1.10:6443",
            "secret-token",
            "worker-1",
            "v1.31.2+k3s1",
        );
        assert!(join_agent_script.contains("K3S_URL=\"https://192.168.1.10:6443\""));
        assert!(join_agent_script.contains("sh -s - agent --node-name=cp-1"));

        let ctx = Context::background();
        let kubeconfig = mgr.get_kubeconfig(&ctx, "192.168.1.10").unwrap();
        assert_eq!(kubeconfig, "server: https://192.168.1.10:6443");
        assert_eq!(mgr.get_version(&ctx).unwrap(), "v1.31.2+k3s1");
    }

    #[test]
    fn test_prereqs_and_doctor_diagnostics() {
        let cfg = Config::load(resolve_data_file("homelab.example.yaml").to_str().unwrap())
            .expect("load homelab.example.yaml");

        let make_healthy_runner = |host: &str| {
            let h = host.to_string();
            Runner::new(&h).with_executor(move |host_name, cmd| {
                if cmd == "echo ok" {
                    Ok("ok".into())
                } else if cmd.contains("which brew") || cmd.contains("/opt/homebrew/bin/brew") {
                    if cmd.contains("--prefix") {
                        Ok("/opt/homebrew".into())
                    } else if cmd.contains("--version") {
                        Ok("limactl version 1.0.0".into())
                    } else {
                        Ok("/opt/homebrew/bin/brew".into())
                    }
                } else if cmd.contains("limactl --version") {
                    Ok("limactl version 1.0.0".into())
                } else if cmd.contains("socket_vmnet") && cmd.contains("test -S") {
                    Ok("/opt/homebrew/var/run/socket_vmnet".into())
                } else if cmd.contains("homebrew.mxcl.socket_vmnet.plist") {
                    Ok("--vmnet-mode=bridged --vmnet-interface=en0".into())
                } else if cmd.contains("route -n get default") {
                    Ok("en0".into())
                } else if cmd.contains("test -f /etc/sudoers.d/lima") {
                    Ok("exists".into())
                } else if cmd == "limactl list --json" {
                    Ok(r#"{"name":"k8s-node","status":"Running"}"#.into())
                } else if cmd.contains("ip -4 addr show lima0") {
                    Ok(format!("192.168.10.{}", host_name.len()))
                } else if cmd.contains("test -f /usr/local/bin/k3s") {
                    Ok("yes".into())
                } else if cmd.contains("ping -c 1") {
                    Ok("1 packets transmitted, 1 received".into())
                } else if cmd.contains("k3s kubectl get nodes") {
                    Ok(
                        "NAME STATUS ROLES\njames-macbook-pro Ready control-plane,etcd,master"
                            .into(),
                    )
                } else {
                    Ok(String::new())
                }
            })
        };

        let ctx = Context::background();
        let runner = make_healthy_runner("james-macbook-pro");
        prereqs::ensure_all_with_runner(&ctx, &runner, "james-macbook-pro", false)
            .expect("prereqs should succeed");

        doctor::run_with_runner_factory(&ctx, &cfg, make_healthy_runner)
            .expect("doctor diagnostics should pass");
    }

    #[test]
    fn test_cluster_lifecycle_operations() {
        let mut cfg = Config::load(resolve_data_file("homelab.example.yaml").to_str().unwrap())
            .expect("load homelab.example.yaml");
        let tmp_dir = env::temp_dir().join(format!("homelab_rs_kube_{}", std::process::id()));
        fs::create_dir_all(&tmp_dir).unwrap();
        let kubeconfig_file = tmp_dir.join("homelab.yaml");
        cfg.cluster.kubeconfig = kubeconfig_file.to_string_lossy().to_string();

        let plan = cluster::format_dry_run_plan(&cfg);
        assert!(plan.contains(
            "[james-macbook-pro] Provision VM → Install K3s server (init) → Label pool=laptop-cp-1"
        ));
        assert!(plan
            .contains("[james-mbp] Provision VM → Install K3s agent → Label pool=laptop-worker-1"));

        let recorded: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
        let rec_clone = Arc::clone(&recorded);

        let make_runner = move |node: &NodeConfig| {
            let rec = Arc::clone(&rec_clone);
            Runner::new(&node.host).with_executor(move |host, cmd| {
                rec.lock().unwrap().push((host.to_string(), cmd.to_string()));
                if cmd.contains("/opt/homebrew/bin/brew") && !cmd.contains("--prefix") {
                    Ok("/opt/homebrew/bin/brew".into())
                } else if cmd.contains("--prefix") {
                    Ok("/opt/homebrew".into())
                } else if cmd.contains("limactl --version") {
                    Ok("limactl version 1.0.0".into())
                } else if cmd.contains("test -f /etc/sudoers.d/lima") {
                    Ok("exists".into())
                } else if cmd.contains("route -n get default") {
                    Ok("en0".into())
                } else if cmd.contains("test -S") {
                    Ok("/opt/homebrew/var/run/socket_vmnet".into())
                } else if cmd.contains("homebrew.mxcl.socket_vmnet.plist") {
                    Ok("--vmnet-mode=bridged --vmnet-interface=en0".into())
                } else if cmd == "limactl list --json" {
                    Ok(r#"{"name":"k8s-node","status":"Running"}"#.into())
                } else if cmd.contains("limactl list k8s-node --json") {
                    Ok(r#"{"name":"k8s-node","status":"Running","cpus":1}"#.into())
                } else if cmd.contains("ip -4 addr show lima0") {
                    Ok("10.0.0.10".into())
                } else if cmd.contains("test -f /usr/local/bin/k3s") {
                    Ok("no".into())
                } else if cmd.contains("node-token && echo ready") {
                    Ok("ready".into())
                } else if cmd.contains("cat /var/lib/rancher/k3s/server/node-token") {
                    Ok("K10token::server:12345".into())
                } else if cmd.contains("systemctl is-active k3s") {
                    Ok("active".into())
                } else if cmd.contains("cat /etc/rancher/k3s/k3s.yaml") {
                    Ok("apiVersion: v1\nclusters:\n- cluster:\n    server: https://127.0.0.1:6443\n".into())
                } else if cmd.contains("k3s kubectl get nodes") {
                    Ok("NAME STATUS ROLES".into())
                } else if cmd.contains("k3s --version") {
                    Ok("k3s version v1.30.0+k3s1 (old)".into())
                } else if cmd.contains("base64 /var/lib/rancher/k3s/server/db/snapshots/") {
                    Ok("c25hcHNob3QtZGF0YQ==".into())
                } else {
                    Ok(String::new())
                }
            })
        };

        let ctx = Context::background();
        cluster::init_with_runner_factory(
            &ctx,
            &cfg,
            InitOptions {
                dry_run: false,
                auto_install: false,
            },
            make_runner.clone(),
        )
        .expect("cluster init should succeed");

        let exported = fs::read_to_string(&kubeconfig_file).expect("kubeconfig should be written");
        assert!(exported.contains("https://10.0.0.10:6443"));

        cluster::apply_with_runner_factory(&ctx, &cfg, false, Duration::ZERO, make_runner.clone())
            .expect("cluster apply should succeed");

        cluster::upgrade_with_runner_factory(
            &ctx,
            &cfg,
            false,
            Duration::ZERO,
            make_runner.clone(),
        )
        .expect("cluster upgrade should succeed");

        cluster::backup_with_runner_factory(
            &ctx,
            &cfg,
            tmp_dir.to_str().unwrap(),
            make_runner.clone(),
        )
        .expect("cluster backup should succeed");

        let snap_path = tmp_dir.join("input-snapshot.db");
        fs::write(&snap_path, b"etcd-snap").unwrap();
        cluster::restore_with_runner_factory(
            &ctx,
            &cfg,
            snap_path.to_str().unwrap(),
            make_runner.clone(),
        )
        .expect("cluster restore should succeed");

        cluster::status_with_runner_factory(&ctx, &cfg, make_runner.clone())
            .expect("cluster status should succeed");

        cluster::remove_with_runner_factory(&ctx, &cfg, "james-mbp", false, make_runner.clone())
            .expect("cluster remove should succeed");

        cluster::destroy_with_runner_factory(&ctx, &cfg, true, false, make_runner)
            .expect("cluster destroy should succeed");
    }

    #[test]
    fn test_cmd_parsing_and_dispatch() {
        let root = RootCmd::new("1.6.1", "abc1234", "2026-09-02");
        assert_eq!(
            root.format_version(),
            "homelab 1.6.1 (commit: abc1234, built: 2026-09-02)"
        );

        let parsed = root
            .parse_args(&[
                "-c".into(),
                "custom.yaml".into(),
                "-v".into(),
                "init".into(),
                "--dry-run".into(),
                "--auto-install".into(),
                "--timeout=45m".into(),
            ])
            .expect("parse init");
        assert_eq!(parsed.config_file, "custom.yaml");
        assert!(parsed.verbose);
        assert_eq!(
            parsed.command,
            Subcommand::Init {
                dry_run: true,
                auto_install: true,
                timeout: Duration::from_secs(45 * 60),
            }
        );

        let parsed_remove = root
            .parse_args(&["remove".into(), "james-mbp".into(), "--dry-run".into()])
            .expect("parse remove");
        assert_eq!(
            parsed_remove.command,
            Subcommand::Remove {
                node_host: "james-mbp".into(),
                dry_run: true,
                timeout: Duration::from_secs(10 * 60),
            }
        );

        assert!(root.parse_args(&["remove".into()]).is_err());
        assert!(root.parse_args(&["restore".into()]).is_err());

        let example_path = resolve_data_file("homelab.example.yaml");
        root.execute_with_args(&[
            "--config".into(),
            example_path.to_string_lossy().into(),
            "init".into(),
            "--dry-run".into(),
        ])
        .expect("execute init --dry-run");

        logging::setup(false);
    }

    #[test]
    fn test_release_please_and_example_config_consistency() {
        let example_path = resolve_data_file("homelab.example.yaml");
        let cfg = Config::load(example_path.to_str().unwrap())
            .expect("homelab.example.yaml must parse and validate cleanly");
        assert_eq!(cfg.cluster.name, "james-homelab");
        assert_eq!(cfg.server_nodes().len(), 3);
        assert_eq!(cfg.agent_nodes().len(), 1);

        let manifest_path = resolve_data_file(".release-please-manifest.json");
        let manifest =
            fs::read_to_string(manifest_path).expect("read .release-please-manifest.json");
        let rp_cfg_path = resolve_data_file("release-please-config.json");
        let rp_cfg = fs::read_to_string(rp_cfg_path).expect("read release-please-config.json");
        let changelog_path = resolve_data_file("CHANGELOG.md");
        let changelog = fs::read_to_string(changelog_path).expect("read CHANGELOG.md");

        let version =
            lima::extract_json_string_field(&manifest.replace('\n', " "), "apps/cli/homelab")
                .expect("extract version from .release-please-manifest.json");
        assert!(!version.is_empty());
        assert!(
            rp_cfg.contains("\"package-name\": \"homelab\""),
            "release-please-config.json must declare homelab package"
        );
        assert!(
            changelog.contains(&format!("## [{}]", version))
                || changelog.contains(&format!("## {}", version)),
            "CHANGELOG.md must contain entry for manifest version {}",
            version
        );
    }
}
