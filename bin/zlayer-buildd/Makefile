# zlayer-buildd Makefile.
#
# Targets:
#   make proto           — regenerate Go bindings from ../../proto/buildah_sidecar.proto
#   make build           — go build ./
#   make vet             — go vet ./...
#   make test            — go test ./...
#   make clean           — remove built artifacts (binary + release/)
#   make release         — cross-compile linux/amd64 + linux/arm64 release binaries
#   make release-amd64   — release build for linux/amd64
#   make release-arm64   — release build for linux/arm64 (needs aarch64-linux-gnu-gcc)
#   make verify-release  — smoke-test the linux/amd64 release binary (--version)
#   make check-deps      — report presence of libgpgme/libassuan/libgpg-error/libseccomp

SHELL := bash
GOPATH_BIN := $(shell go env GOPATH)/bin
export PATH := $(GOPATH_BIN):$(PATH)

REPO_ROOT := $(abspath ../..)
PROTO_DIR := $(REPO_ROOT)/proto
PROTO_FILE := $(PROTO_DIR)/buildah_sidecar.proto
PB_OUT := internal/pb

GO_MODULE := github.com/zorpxinc/zlayer/zlayer-buildd
PROTOC_GEN_GO_VERSION := v1.36.11
PROTOC_GEN_GO_GRPC_VERSION := v1.6.2

RELEASE_DIR := release
VERSION ?= $(shell git describe --tags --dirty --always 2>/dev/null || echo dev)

# Build tags applied uniformly to all release builds. Drop the
# devicemapper and btrfs graphdrivers (we only support overlay + vfs from
# the sidecar), and switch to pure-Go OpenPGP to thin out the dynamic-link
# footprint.
BUILD_TAGS := exclude_graphdriver_devicemapper exclude_graphdriver_btrfs containers_image_openpgp

# Linker flags injecting the version constant.
LDFLAGS := -ldflags "-s -w -X main.sidecarVersion=$(VERSION)"

.PHONY: proto build vet test clean tools release release-amd64 release-arm64 verify-release check-deps

tools:
	@command -v protoc-gen-go      >/dev/null || go install google.golang.org/protobuf/cmd/protoc-gen-go@$(PROTOC_GEN_GO_VERSION)
	@command -v protoc-gen-go-grpc >/dev/null || go install google.golang.org/grpc/cmd/protoc-gen-go-grpc@$(PROTOC_GEN_GO_GRPC_VERSION)

proto: tools
	@mkdir -p $(PB_OUT)
	protoc \
	    --proto_path=$(PROTO_DIR) \
	    --go_out=. \
	    --go_opt=Mbuildah_sidecar.proto=$(GO_MODULE)/$(PB_OUT) \
	    --go_opt=module=$(GO_MODULE) \
	    --go-grpc_out=. \
	    --go-grpc_opt=Mbuildah_sidecar.proto=$(GO_MODULE)/$(PB_OUT) \
	    --go-grpc_opt=module=$(GO_MODULE) \
	    $(PROTO_FILE)
	@test -f $(PB_OUT)/buildah_sidecar.pb.go        || (echo "missing: $(PB_OUT)/buildah_sidecar.pb.go"      && exit 1)
	@test -f $(PB_OUT)/buildah_sidecar_grpc.pb.go   || (echo "missing: $(PB_OUT)/buildah_sidecar_grpc.pb.go" && exit 1)

build: proto
	go build -o ./zlayer-buildd ./

vet: proto
	go vet ./...

test: proto
	go test ./...

clean:
	rm -f ./zlayer-buildd $(PB_OUT)/*.pb.go
	rm -rf $(RELEASE_DIR)

# Cross-compile + release targets ----------------------------------------

release: release-amd64 release-arm64

# Native-architecture release build. Always cgo-enabled.
release-amd64: proto
	@mkdir -p $(RELEASE_DIR)
	GOOS=linux GOARCH=amd64 CGO_ENABLED=1 \
	    go build -tags "$(BUILD_TAGS)" $(LDFLAGS) \
	    -o $(RELEASE_DIR)/zlayer-buildd-linux-amd64 ./

# Cross-arch release build. cgo is gated behind a cross-compiler; we
# detect aarch64 toolchain availability and emit a clear error otherwise.
release-arm64: proto
	@if [ "$$(uname -m)" = "x86_64" ] && ! command -v aarch64-linux-gnu-gcc >/dev/null 2>&1; then \
	    echo "release-arm64 needs an aarch64 cross-toolchain (apt: gcc-aarch64-linux-gnu; arch: aarch64-linux-gnu-gcc)"; \
	    exit 1; \
	fi
	@mkdir -p $(RELEASE_DIR)
	GOOS=linux GOARCH=arm64 CGO_ENABLED=1 \
	    CC=$$( [ "$$(uname -m)" = "aarch64" ] && echo cc || echo aarch64-linux-gnu-gcc ) \
	    go build -tags "$(BUILD_TAGS)" $(LDFLAGS) \
	    -o $(RELEASE_DIR)/zlayer-buildd-linux-arm64 ./

# Verify the release binary boots and prints --version. Run AFTER release-*.
verify-release: release-amd64
	@$(RELEASE_DIR)/zlayer-buildd-linux-amd64 --version | jq -e '.sidecar_version,.buildah_version,.go_version'
	@echo "release-amd64 ok"

# System-level build dependencies. Reports the libraries we need; does NOT
# install them — that's the operator's job.
#
# Some distros expose pkg-config modules with the `lib` prefix (Debian:
# libgpgme, libgpg-error) while others drop it (Arch: gpgme, gpg-error).
# We accept either spelling.
check-deps:
	@echo "zlayer-buildd build-time dynamic dependencies (CGO):"
	@for spec in "libgpgme:gpgme" "libassuan:assuan" "libgpg-error:gpg-error" "libseccomp:seccomp"; do \
	    primary=$${spec%%:*}; \
	    alt=$${spec##*:}; \
	    if pkg-config --exists "$$primary" 2>/dev/null; then \
	        printf "  %s: ok (%s)\n" "$$primary" "$$(pkg-config --modversion $$primary)"; \
	    elif pkg-config --exists "$$alt" 2>/dev/null; then \
	        printf "  %s: ok (%s, via pkg-config name '%s')\n" "$$primary" "$$(pkg-config --modversion $$alt)" "$$alt"; \
	    else \
	        printf "  %s: MISSING (apt install libgpgme-dev libassuan-dev libgpg-error-dev libseccomp-dev; arch: pacman -S gpgme libassuan libgpg-error libseccomp)\n" "$$primary"; \
	    fi; \
	done
