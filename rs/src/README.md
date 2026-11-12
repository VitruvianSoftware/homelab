# Src

## Overview
Component package providing implementation for `apps/cli/vitruvian-homelab/rs/src`.

## Interface
- `[cluster.rs:InitOptions]`
- `[cluster.rs:init]`
- `[cluster.rs:init_with_runner_factory]`
- `[cluster.rs:apply]`
- `[cluster.rs:apply_with_runner_factory]`
- `[cluster.rs:join]`
- `[cluster.rs:join_with_runner_factory]`
- `[cluster.rs:remove]`
- `[cluster.rs:remove_with_runner_factory]`
- `[cluster.rs:destroy]`
- `[cluster.rs:destroy_with_runner_factory]`
- `[cluster.rs:status]`
- `[cluster.rs:status_with_runner_factory]`
- `[cluster.rs:upgrade]`
- `[cluster.rs:upgrade_with_runner_factory]`
- `[cluster.rs:backup]`
- `[cluster.rs:backup_with_runner_factory]`
- `[cluster.rs:restore]`
- `[cluster.rs:restore_with_runner_factory]`
- `[cluster.rs:new_runner]`

## Invariants
- [lib.rs:83]: `panic!("could not locate test data file: {}", rel);`
- [lib.rs:138]: `assert!(res.is_err(), "expected error for missing file");`
- [lib.rs:145]: `assert!(res.is_err(), "expected error for malformed YAML");`

## Side Effects
- [cluster.rs:85]: `handles.push(thread::spawn(move || {`
- [cluster.rs:111]: `handles.push(thread::spawn(move || -> Result<(), String> {`
- [k3s.rs:94]: `"curl -sfL https://get.k3s.io | {}INSTALL_K3S_EXEC=\"server\" sh -s - --clust...`
- [k3s.rs:161]: `"curl -sfL https://get.k3s.io | {}INSTALL_K3S_SKIP_START=true K3S_TOKEN={} IN...`
- [k3s.rs:278]: `"curl -sfL https://get.k3s.io | {}K3S_TOKEN={} K3S_URL={} sh -s - agent --nod...`
- [lima.rs:136]: `apt-get update -qq && apt-get install -y -qq curl open-iscsi nfs-common`
- [prereqs.rs:116]: `r#"NONINTERACTIVE=1 /bin/bash -c "$(curl -fsSL https://raw.githubusercontent....`
- [prereqs.rs:232]: `<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DT...`
- [remote.rs:176]: `pub fn with_executor<F>(mut self, exec: F) -> Self`
- [remote.rs:180]: `self.executor = Some(Arc::new(exec));`

## Verification
- Run: `bazel test //apps/cli/vitruvian-homelab/rs/src/...`
