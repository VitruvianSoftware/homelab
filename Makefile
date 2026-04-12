BINARY   := homelab
MODULE   := github.com/VitruvianSoftware/homelab
VERSION  := $(shell git describe --tags --always --dirty 2>/dev/null || echo dev)
COMMIT   := $(shell git rev-parse --short HEAD 2>/dev/null || echo none)
DATE     := $(shell date -u +%Y-%m-%dT%H:%M:%SZ)
LDFLAGS  := -s -w \
  -X main.version=$(VERSION) \
  -X main.commit=$(COMMIT) \
  -X main.date=$(DATE)

.PHONY: build install clean test lint fmt help

## build: Build the binary for the current platform.
build:
	go build -ldflags "$(LDFLAGS)" -o $(BINARY) ./cmd/homelab

## install: Build and install to $GOPATH/bin.
install:
	go install -ldflags "$(LDFLAGS)" ./cmd/homelab

## test: Run all tests.
test:
	go test -race -cover ./...

## lint: Run golangci-lint.
lint:
	golangci-lint run ./...

## fmt: Format all Go source files.
fmt:
	gofmt -s -w .
	goimports -w .

## clean: Remove build artifacts.
clean:
	rm -f $(BINARY)
	rm -rf dist/

## help: Show this help message.
help:
	@echo "Usage: make [target]"
	@echo ""
	@awk '/^## / { sub(/^## /, ""); print }' $(MAKEFILE_LIST) | column -t -s ':'
