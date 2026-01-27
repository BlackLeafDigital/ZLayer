.PHONY: all check test build release clean fmt lint ci docs install

# Default target
all: check test build

# Check compilation
check:
	cargo check --workspace --all-targets

# Run all tests
test:
	cargo test --workspace

# Build debug
build:
	cargo build --workspace

# Build release
release:
	cargo build --release --package runtime

# Clean build artifacts
clean:
	cargo clean

# Format code
fmt:
	cargo fmt --all

# Check formatting (CI mode)
fmt-check:
	cargo fmt --all -- --check

# Lint code
lint:
	cargo clippy --workspace --all-targets -- -D warnings

# Full CI pipeline locally
ci: fmt-check lint check test
	@echo "CI checks passed!"

# Run specific crate tests
test-scheduler:
	cargo test --package scheduler

test-api:
	cargo test --package api

test-observability:
	cargo test --package observability

test-agent:
	cargo test --package agent

# E2E tests requiring root (youki/libcontainer)
test-e2e:
	@echo "Cleaning up leftover state..."
	sudo rm -rf /tmp/zlayer-youki-e2e-test/state/* 2>/dev/null || true
	@echo "Running E2E tests with root privileges..."
	sudo -E env "PATH=$(HOME)/.cargo/bin:$(PATH)" \
		cargo test --package agent --test youki_e2e -- --nocapture --test-threads=1

# Run specific E2E test (usage: make test-e2e-single TEST=test_cleanup_state_directory)
test-e2e-single:
	@echo "Cleaning up leftover state..."
	sudo rm -rf /tmp/zlayer-youki-e2e-test/state/* 2>/dev/null || true
	@echo "Running E2E test: $(TEST)"
	sudo -E env "PATH=$(HOME)/.cargo/bin:$(PATH)" \
		cargo test --package agent --test youki_e2e $(TEST) -- --nocapture

test-spec:
	cargo test --package spec

test-proxy:
	cargo test --package proxy

test-overlay:
	cargo test --package overlay

test-registry:
	cargo test --package registry

test-runtime:
	cargo test --package runtime

# Test with specific features
test-features:
	cargo test --package runtime --no-default-features --features join
	cargo test --package runtime --no-default-features --features serve
	cargo test --package runtime --no-default-features --features deploy
	cargo test --package runtime --features full

# Generate documentation
docs:
	cargo doc --workspace --no-deps --open

# Generate documentation without opening
docs-build:
	cargo doc --workspace --no-deps

# Install locally
install:
	cargo install --path bin/runtime

# Install in development mode (faster builds)
install-dev:
	cargo install --path bin/runtime --debug

# Run the API server (for development)
run-server:
	cargo run --package runtime -- serve

# Run with verbose logging
run-server-verbose:
	cargo run --package runtime -- -vv serve

# Validate example spec
validate-example:
	cargo run --package runtime -- validate examples/basic-deployment.yaml

# Watch mode for development (requires cargo-watch)
watch:
	cargo watch -x check -x 'test --workspace'

# Security audit (requires cargo-audit)
audit:
	cargo audit

# Update dependencies
update:
	cargo update

# Show outdated dependencies (requires cargo-outdated)
outdated:
	cargo outdated

# Generate schema for spec (if schemars is used)
schema:
	cargo run --package runtime --features full -- spec dump examples/basic-deployment.yaml --format json

# Help target
help:
	@echo "ZLayer Development Commands"
	@echo "==========================="
	@echo ""
	@echo "Build targets:"
	@echo "  make all          - Check, test, and build (default)"
	@echo "  make check        - Check compilation"
	@echo "  make build        - Build debug binaries"
	@echo "  make release      - Build release binaries"
	@echo "  make clean        - Clean build artifacts"
	@echo ""
	@echo "Quality targets:"
	@echo "  make fmt          - Format code"
	@echo "  make fmt-check    - Check formatting"
	@echo "  make lint         - Run clippy lints"
	@echo "  make ci           - Run full CI pipeline locally"
	@echo ""
	@echo "Test targets:"
	@echo "  make test         - Run all tests"
	@echo "  make test-<crate> - Run tests for specific crate"
	@echo "  make test-e2e     - Run E2E tests (requires sudo)"
	@echo "  make test-e2e-single TEST=name - Run single E2E test"
	@echo "  make test-features- Test feature flag combinations"
	@echo ""
	@echo "Documentation:"
	@echo "  make docs         - Generate and open documentation"
	@echo "  make docs-build   - Generate documentation"
	@echo ""
	@echo "Development:"
	@echo "  make run-server   - Start the API server"
	@echo "  make watch        - Watch mode (requires cargo-watch)"
	@echo "  make install      - Install binaries locally"
	@echo ""
	@echo "Maintenance:"
	@echo "  make audit        - Security audit (requires cargo-audit)"
	@echo "  make update       - Update dependencies"
	@echo "  make outdated     - Show outdated deps (requires cargo-outdated)"
