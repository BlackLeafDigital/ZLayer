// Package main is the entry point for zlayer-buildd, the buildah sidecar
// daemon. See ../../proto/buildah_sidecar.proto for the gRPC schema.
//
// The daemon is launched on demand by the Rust BuildahSidecarBackend; on
// startup it binds an mTLS gRPC listener (default 127.0.0.1:0) and prints a
// single `LISTENING host:port` line to stdout so the parent can dial in.
// Everything after that line is structured logging on stderr.
package main

import (
	"context"
	"crypto/tls"
	"crypto/x509"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"log/slog"
	"net"
	"os"
	"os/signal"
	"runtime"
	"runtime/debug"
	"strings"
	"syscall"
	"time"

	pb "github.com/zorpxinc/zlayer/zlayer-buildd/internal/pb"
	"github.com/zorpxinc/zlayer/zlayer-buildd/internal/server"

	"go.podman.io/buildah"
	"go.podman.io/storage"
	"go.podman.io/storage/pkg/unshare"

	"google.golang.org/grpc"
	"google.golang.org/grpc/credentials"
	"google.golang.org/grpc/health"
	healthpb "google.golang.org/grpc/health/grpc_health_v1"
)

// stopReason carries a human-readable reason for triggering daemon
// shutdown. The signal handler and idle loop both push instances onto a
// shared channel; the central stopper reads one and runs GracefulStop.
type stopReason struct{ why string }

// sidecarVersion is overridable at link time via
// `-ldflags "-X main.sidecarVersion=v0.1.0"`. When empty we fall back to the
// Go module's build info (`(devel)` for `go build` without VCS metadata).
var sidecarVersion = ""

// buildahVersion is a const baked at compile time. The buildah package
// exposes its own version string via buildah.Version but it is not exported
// as a string constant across all releases; we mirror the go.mod pin so the
// --version output stays stable.
const buildahVersion = "v1.44.0"

func main() {
	// MUST be the very first call. Returns true when the process was
	// re-execed for a containers/storage helper (e.g. the unshare or
	// chrootarchive trampoline). We exit immediately in that case so the
	// helper can run without us touching flags, stdout, or sockets.
	if buildah.InitReexec() {
		return
	}

	// Rootless support: when running as a non-root uid, re-exec ourselves
	// inside a new user namespace where the calling uid maps to root.
	// Without this, buildah's `BuildDockerfiles` always fails when it
	// tries to chown the build-context overlay scaffolding to uid 0
	// under /var/tmp/buildah-context-* (EPERM). This is a no-op when
	// the process is already running as real root (euid 0 with rootless
	// UID > 0 means we already re-execed; bare euid 0 means we don't need
	// to). Mirrors what the upstream `buildah` CLI does in its `main()`.
	unshare.MaybeReexecUsingUserNamespace(false)

	bind := flag.String("bind", "127.0.0.1:0", "host:port to bind (':0' for OS-allocated port)")
	tlsCA := flag.String("tls-ca", "", "PEM CA cert used to verify client certs (required)")
	tlsCert := flag.String("tls-cert", "", "PEM server cert chain (required)")
	tlsKey := flag.String("tls-key", "", "PEM server private key (required)")
	idleSecs := flag.Uint("idle-secs", 30, "shut down after this many seconds of RPC inactivity (0 disables)")
	maxConcurrent := flag.Int("max-concurrent", 1, "cap on concurrent Build RPCs")
	storageDriver := flag.String("storage-driver", "", "override containers/storage driver (overlay, vfs, btrfs, ...)")
	storageRoot := flag.String("storage-root", "", "override containers/storage GraphRoot")
	storageRunroot := flag.String("storage-runroot", "", "override containers/storage RunRoot")
	logLevel := flag.String("log-level", "info", "log level: error, warn, info, debug")
	versionFlag := flag.Bool("version", false, "print version JSON and exit")
	flag.Parse()

	if *versionFlag {
		out := map[string]string{
			"sidecar_version": resolveSidecarVersion(),
			"buildah_version": buildahVersion,
			"go_version":      runtime.Version(),
		}
		// Stable, single-line JSON for easy parsing by the Rust caller.
		enc := json.NewEncoder(os.Stdout)
		if err := enc.Encode(out); err != nil {
			fmt.Fprintf(os.Stderr, "zlayer-buildd: failed to encode version: %v\n", err)
			os.Exit(1)
		}
		return
	}

	logger := newLogger(*logLevel)
	slog.SetDefault(logger)

	if *tlsCA == "" || *tlsCert == "" || *tlsKey == "" {
		logger.Error("missing required TLS flags", "tls_ca", *tlsCA, "tls_cert", *tlsCert, "tls_key", *tlsKey)
		fmt.Fprintln(os.Stderr, "zlayer-buildd: --tls-ca, --tls-cert and --tls-key are required")
		os.Exit(2)
	}

	tlsConfig, err := loadTLSConfig(*tlsCA, *tlsCert, *tlsKey)
	if err != nil {
		logger.Error("failed to load TLS config", "error", err)
		os.Exit(1)
	}

	// Storage options — always initialize. The Build RPC requires a live
	// containers/storage Store on the server, so we cannot defer this. Flags
	// override the rootless/root defaults that go.podman.io/storage picks
	// based on uid + /etc/containers/storage.conf.
	storeOpts, derr := storage.DefaultStoreOptions()
	if derr != nil {
		logger.Warn("could not load default storage options; falling back to zero value", "error", derr)
		storeOpts = storage.StoreOptions{}
	}
	if *storageRoot != "" {
		storeOpts.GraphRoot = *storageRoot
	}
	if *storageRunroot != "" {
		storeOpts.RunRoot = *storageRunroot
	}
	if *storageDriver != "" {
		storeOpts.GraphDriverName = *storageDriver
	}
	store, err := storage.GetStore(storeOpts)
	if err != nil {
		logger.Error("failed to open containers/storage store", "error", err,
			"graph_root", storeOpts.GraphRoot, "run_root", storeOpts.RunRoot,
			"driver", storeOpts.GraphDriverName)
		os.Exit(1)
	}
	logger.Info("containers/storage store initialised",
		"graph_root", storeOpts.GraphRoot,
		"run_root", storeOpts.RunRoot,
		"driver", storeOpts.GraphDriverName,
	)
	// Flush layers to disk on graceful exit. Shutdown(false) refuses if any
	// containers are still mounted, which is the safe default — a true value
	// would unmount them out from under any in-flight build.
	defer func() {
		if _, sErr := store.Shutdown(false); sErr != nil {
			logger.Warn("store.Shutdown returned error", "error", sErr)
		}
	}()

	lis, err := net.Listen("tcp", *bind)
	if err != nil {
		logger.Error("failed to listen", "bind", *bind, "error", err)
		os.Exit(1)
	}
	tcpAddr, ok := lis.Addr().(*net.TCPAddr)
	if !ok {
		logger.Error("listener returned non-TCP address", "addr", lis.Addr().String())
		os.Exit(1)
	}

	// The Rust lifecycle manager reads this single line off stdout to learn
	// the actually-bound port. Format MUST be exactly `LISTENING host:port`
	// followed by a newline. All subsequent output goes to stderr via slog.
	if _, werr := fmt.Fprintf(os.Stdout, "LISTENING %s:%d\n", tcpAddr.IP.String(), tcpAddr.Port); werr != nil {
		logger.Error("failed to write handshake line", "error", werr)
		os.Exit(1)
	}
	if serr := os.Stdout.Sync(); serr != nil {
		// Sync on a pipe can fail on some platforms with ENOTSUP; log but
		// don't abort — the line is already written.
		logger.Debug("stdout sync returned non-fatal error", "error", serr)
	}

	srvOpts := server.Options{
		Store:         store,
		MaxConcurrent: *maxConcurrent,
		SystemContext: nil,
	}
	srv := server.New(srvOpts)

	grpcServer := grpc.NewServer(grpc.Creds(credentials.NewTLS(tlsConfig)))
	pb.RegisterBuildServiceServer(grpcServer, srv)

	// Standard gRPC health service so external probes (grpc_health_probe,
	// k8s, etc.) can verify readiness without speaking BuildService.
	healthSrv := health.NewServer()
	healthSrv.SetServingStatus("", healthpb.HealthCheckResponse_SERVING)
	healthSrv.SetServingStatus("zlayer.buildd.v1.BuildService", healthpb.HealthCheckResponse_SERVING)
	healthpb.RegisterHealthServer(grpcServer, healthSrv)

	logger.Info("zlayer-buildd ready",
		"bind", fmt.Sprintf("%s:%d", tcpAddr.IP.String(), tcpAddr.Port),
		"idle_secs", *idleSecs,
		"max_concurrent", *maxConcurrent,
		"graph_root", storeOpts.GraphRoot,
		"run_root", storeOpts.RunRoot,
		"driver", storeOpts.GraphDriverName,
		"sidecar_version", resolveSidecarVersion(),
	)

	// Signal handling: SIGINT/SIGTERM → graceful stop with a 10s hard-stop
	// fallback. We funnel the trigger reason through a small struct so the
	// idle loop can share the same shutdown path.
	stopCh := make(chan stopReason, 2)

	sigCh := make(chan os.Signal, 1)
	signal.Notify(sigCh, syscall.SIGINT, syscall.SIGTERM)
	go func() {
		s := <-sigCh
		stopCh <- stopReason{why: "signal:" + s.String()}
	}()
	defer signal.Stop(sigCh)

	// Idle-shutdown loop. Disabled when --idle-secs=0. Wakes every 5s and
	// triggers graceful stop once the last activity timestamp ages past the
	// configured window.
	ctx, cancelIdle := context.WithCancel(context.Background())
	defer cancelIdle()
	if *idleSecs > 0 {
		idleDuration := time.Duration(*idleSecs) * time.Second
		go runIdleLoop(ctx, logger, srv, idleDuration, stopCh)
	}

	// One-shot stopper: graceful first, then hard-stop after 10s if the
	// gRPC server hasn't drained.
	go func() {
		reason := <-stopCh
		logger.Info("shutting down", "reason", reason.why)
		done := make(chan struct{})
		go func() {
			grpcServer.GracefulStop()
			close(done)
		}()
		select {
		case <-done:
			logger.Info("graceful stop complete")
		case <-time.After(10 * time.Second):
			logger.Warn("graceful stop timed out; forcing hard stop")
			grpcServer.Stop()
		}
	}()

	if serveErr := grpcServer.Serve(lis); serveErr != nil && !errors.Is(serveErr, grpc.ErrServerStopped) {
		logger.Error("server exited with error", "error", serveErr)
		os.Exit(1)
	}
	logger.Info("server stopped")
}

// runIdleLoop polls the server's last-activity timestamp every 5 seconds.
// Once the gap exceeds idle, it pushes onto stopCh and returns. The first
// reading is taken after server.New, which seeds the timestamp to "now".
func runIdleLoop(
	ctx context.Context,
	logger *slog.Logger,
	srv *server.Server,
	idle time.Duration,
	stopCh chan<- stopReason,
) {
	t := time.NewTicker(5 * time.Second)
	defer t.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-t.C:
			lastNs := srv.LastActivityUnixNanos()
			if lastNs == 0 {
				continue
			}
			gap := time.Since(time.Unix(0, lastNs))
			if gap < idle {
				continue
			}
			// A long pull or compile can stall activity touches well past
			// the idle window without the build being abandoned. Defer
			// shutdown while any Build RPC is still in flight; the
			// build's own defer-touchActivity at completion will then
			// restart the idle clock from a fresh baseline.
			if n := srv.InflightCount(); n > 0 {
				logger.Debug("idle gap exceeded but build in flight; deferring shutdown",
					"idle_seconds", idle.Seconds(),
					"observed_gap_seconds", gap.Seconds(),
					"inflight", n)
				continue
			}
			logger.Info("idle shutdown",
				"idle_seconds", idle.Seconds(),
				"observed_gap_seconds", gap.Seconds())
			stopCh <- stopReason{why: fmt.Sprintf("idle:%.0fs", idle.Seconds())}
			return
		}
	}
}

// loadTLSConfig builds the server's mTLS configuration. ClientAuth is set to
// RequireAndVerifyClientCert so peers without a CA-signed cert get rejected
// during the TLS handshake.
func loadTLSConfig(caPath, certPath, keyPath string) (*tls.Config, error) {
	caBytes, err := os.ReadFile(caPath)
	if err != nil {
		return nil, fmt.Errorf("read CA: %w", err)
	}
	pool := x509.NewCertPool()
	if !pool.AppendCertsFromPEM(caBytes) {
		return nil, fmt.Errorf("no PEM certificates found in %s", caPath)
	}
	cert, err := tls.LoadX509KeyPair(certPath, keyPath)
	if err != nil {
		return nil, fmt.Errorf("load server keypair: %w", err)
	}
	return &tls.Config{
		Certificates: []tls.Certificate{cert},
		ClientCAs:    pool,
		ClientAuth:   tls.RequireAndVerifyClientCert,
		MinVersion:   tls.VersionTLS13,
	}, nil
}

// newLogger constructs a stderr JSON slog.Logger at the requested level.
// Unknown level strings degrade to info so a typo in --log-level never
// silences logging entirely.
func newLogger(level string) *slog.Logger {
	var lvl slog.Level
	switch strings.ToLower(level) {
	case "error":
		lvl = slog.LevelError
	case "warn", "warning":
		lvl = slog.LevelWarn
	case "debug":
		lvl = slog.LevelDebug
	default:
		lvl = slog.LevelInfo
	}
	h := slog.NewJSONHandler(os.Stderr, &slog.HandlerOptions{Level: lvl})
	return slog.New(h).With("component", "zlayer-buildd")
}

// resolveSidecarVersion returns the build-time version when set, otherwise
// it falls back to the Go module's build info. For `go build` without VCS
// metadata this is `(devel)`, which is the standard signal for a dev build.
func resolveSidecarVersion() string {
	if sidecarVersion != "" {
		return sidecarVersion
	}
	if info, ok := debug.ReadBuildInfo(); ok && info.Main.Version != "" {
		return info.Main.Version
	}
	return "(unknown)"
}
