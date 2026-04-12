// Package config handles loading, parsing, and validating the homelab cluster
// configuration from YAML files.
package config

import (
	"fmt"
	"os"

	"gopkg.in/yaml.v3"
)

// Config is the top-level configuration for a homelab cluster.
type Config struct {
	Cluster ClusterConfig `yaml:"cluster"`
	Nodes   []NodeConfig  `yaml:"nodes"`
}

// ClusterConfig holds cluster-wide settings.
type ClusterConfig struct {
	Name       string `yaml:"name"`
	K3sVersion string `yaml:"k3sVersion"`
	Kubeconfig string `yaml:"kubeconfig"`
}

// NodeConfig describes a single node in the cluster.
type NodeConfig struct {
	Host string     `yaml:"host"`
	Role string     `yaml:"role"` // "server" or "agent"
	Pool string     `yaml:"pool"`
	VM   VMConfig   `yaml:"vm"`
}

// VMConfig describes the resource allocation for a Lima VM.
type VMConfig struct {
	CPUs   int    `yaml:"cpus"`
	Memory string `yaml:"memory"`
	Disk   string `yaml:"disk"`
}

// Load reads and parses a config file from the given path.
func Load(path string) (*Config, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, fmt.Errorf("reading config file %s: %w", path, err)
	}

	var cfg Config
	if err := yaml.Unmarshal(data, &cfg); err != nil {
		return nil, fmt.Errorf("parsing config file %s: %w", path, err)
	}

	if err := cfg.Validate(); err != nil {
		return nil, fmt.Errorf("validating config: %w", err)
	}

	return &cfg, nil
}

// ServerNodes returns only the nodes with role "server".
func (c *Config) ServerNodes() []NodeConfig {
	var servers []NodeConfig
	for _, n := range c.Nodes {
		if n.Role == "server" {
			servers = append(servers, n)
		}
	}
	return servers
}

// AgentNodes returns only the nodes with role "agent".
func (c *Config) AgentNodes() []NodeConfig {
	var agents []NodeConfig
	for _, n := range c.Nodes {
		if n.Role == "agent" {
			agents = append(agents, n)
		}
	}
	return agents
}

// InitNode returns the first server node, which is used to bootstrap the cluster.
func (c *Config) InitNode() NodeConfig {
	return c.ServerNodes()[0]
}
