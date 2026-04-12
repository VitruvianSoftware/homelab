# homelab

A CLI tool for declaratively provisioning and managing multi-node Kubernetes homelab clusters on macOS using Lima VZ and K3s.

## Overview

`homelab` lets you define your cluster topology in a simple YAML config and handles the full lifecycle: provisioning Lima VMs with Apple's native Virtualization framework, bootstrapping a highly available K3s cluster, and managing node pools across multiple physical machines.

## Features

- **Declarative Configuration** — Define nodes, roles, specs, and pools in a single YAML file
- **Idempotent Operations** — Safe to run repeatedly; only applies changes needed to reach desired state
- **High Availability** — Supports multi-node control plane with embedded etcd
- **Multi-Architecture** — Seamlessly handles ARM64 and AMD64 nodes in the same cluster
- **Health Diagnostics** — Built-in `doctor` command for pre-flight checks and cluster health
- **Day-2 Operations** — Add/remove node pools, scale up, tear down

## Usage

```bash
# Check prerequisites and cluster health
homelab doctor

# Bootstrap a new cluster
homelab init

# Add new nodes to an existing cluster
homelab join

# Remove a node
homelab remove <node>

# Show cluster status
homelab status

# Tear down everything
homelab destroy
```

## Configuration

```yaml
cluster:
  name: my-homelab
  k3sVersion: "v1.31.2+k3s1"
  kubeconfig: "~/.kube/homelab.yaml"

nodes:
  - host: my-mac-1
    role: server
    pool: cp-1
    vm:
      cpus: 2
      memory: 4GiB
      disk: 30GiB

  - host: my-mac-2
    role: agent
    pool: worker-1
    vm:
      cpus: 4
      memory: 8GiB
      disk: 50GiB
```

## Requirements

- macOS with Apple Virtualization framework support
- [Lima](https://lima-vm.io/) with `vz` VM type support
- [socket_vmnet](https://github.com/lima-vm/socket_vmnet) for bridged networking
- SSH access to all target machines

## License

Apache License 2.0
