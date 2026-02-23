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
	cargo build --release --package zlayer

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
	cargo test --package zlayer-scheduler

test-api:
	cargo test --package zlayer-api

test-observability:
	cargo test --package zlayer-observability

test-agent:
	cargo test --package zlayer-agent

test-manager:
	cargo test --package zlayer-manager --features ssr

# E2E tests requiring root (youki/libcontainer)
test-e2e:
	@echo "Cleaning up leftover state..."
	sudo rm -rf /tmp/zlayer-youki-e2e-test/state/* 2>/dev/null || true
	@echo "Running E2E tests with root privileges..."
	sudo -E env "PATH=$(HOME)/.cargo/bin:$(PATH)" \
		cargo test --package zlayer-agent --test youki_e2e -- --nocapture --test-threads=1

# Run specific E2E test (usage: make test-e2e-single TEST=test_cleanup_state_directory)
test-e2e-single:
	@echo "Cleaning up leftover state..."
	sudo rm -rf /tmp/zlayer-youki-e2e-test/state/* 2>/dev/null || true
	@echo "Running E2E test: $(TEST)"
	sudo -E env "PATH=$(HOME)/.cargo/bin:$(PATH)" \
		cargo test --package zlayer-agent --test youki_e2e $(TEST) -- --nocapture

test-spec:
	cargo test --package zlayer-spec

test-proxy:
	cargo test --package zlayer-proxy

test-overlay:
	cargo test --package zlayer-overlay

test-registry:
	cargo test --package zlayer-registry

test-runtime:
	cargo test --package zlayer

# Test with specific features
test-features:
	cargo test --package zlayer --no-default-features --features full
	cargo test --package zlayer --features full

# macOS Sandbox E2E tests
test-macos-sandbox:
	@echo "Setting up macOS sandbox E2E environment..."
	rm -rf /tmp/zlayer-macos-sandbox-e2e-test 2>/dev/null || true
	mkdir -p /tmp/zlayer-macos-sandbox-e2e-test/{data,logs}
	mkdir -p /tmp/zlayer-macos-sandbox-e2e-test/data/{containers,images}
	@echo "Running macOS Sandbox E2E tests..."
	cargo test --package zlayer-agent --test macos_sandbox_e2e -- --nocapture

# MPS GPU smoke test (requires Apple Silicon + PyTorch via uv)
test-mps-smoke:
	@echo "Running MPS GPU smoke test..."
	cargo test --package zlayer-agent --test macos_mps_smoke -- --nocapture

# Generate documentation
docs:
	cargo doc --workspace --no-deps --open

# Generate documentation without opening
docs-build:
	cargo doc --workspace --no-deps

# Install locally
install:
	cargo install --path bin/zlayer

# Install in development mode (faster builds)
install-dev:
	cargo install --path bin/zlayer --debug

# Run the API server (for development)
run-server:
	cargo run --package zlayer -- serve

# Run with verbose logging
run-server-verbose:
	cargo run --package zlayer -- -vv serve

# Validate example spec
validate-example:
	cargo run --package zlayer -- validate examples/basic-deployment.yaml

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
	cargo run --package zlayer --features full -- spec dump examples/basic-deployment.yaml --format json

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
	@echo "  make test-macos-sandbox - Run macOS sandbox E2E tests"
	@echo "  make test-mps-smoke - Run MPS GPU smoke test (Apple Silicon)"
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
