package server

import (
	"context"
	"runtime"
	"runtime/debug"

	pb "github.com/zorpxinc/zlayer/zlayer-buildd/internal/pb"
	"go.podman.io/buildah"
)

// Inspect returns daemon metadata: sidecar version, buildah version,
// supported platforms (the host's GOOS/GOARCH plus any cross-arch the host
// can natively execute), and storage configuration.
func (s *Server) Inspect(_ context.Context, _ *pb.InspectRequest) (*pb.InspectResponse, error) {
	s.touchActivity()
	defer s.touchActivity()

	resp := &pb.InspectResponse{
		SidecarVersion:     resolveSidecarVersion(),
		BuildahVersion:     buildahVersion(),
		SupportedPlatforms: hostSupportedPlatforms(),
	}

	if store := s.opts.Store; store != nil {
		resp.StorageDriver = store.GraphDriverName()
		resp.StorageRoot = store.GraphRoot()
	}
	return resp, nil
}

// Health reports whether the daemon is currently serving. Distinct from
// gRPC's standard google.golang.org/grpc/health/grpc_health_v1 server
// registered separately in main(); this is the BuildService-specific
// health endpoint.
func (s *Server) Health(_ context.Context, _ *pb.HealthRequest) (*pb.HealthResponse, error) {
	s.touchActivity()
	defer s.touchActivity()

	status := pb.HealthResponse_SERVING
	detail := ""
	if s.opts.Store == nil {
		status = pb.HealthResponse_NOT_SERVING
		detail = "containers/storage store is not initialized"
	}
	return &pb.HealthResponse{
		Status: status,
		Detail: detail,
	}, nil
}

// buildahVersion returns the build-time version of the go.podman.io/buildah
// dependency. We get it from `debug.ReadBuildInfo()` rather than baking the
// pin into source code, so a `go get -u` doesn't silently lie about which
// version is linked. Falls back to the package's exported Version constant
// when build info isn't available (e.g. some test binaries).
func buildahVersion() string {
	if info, ok := debug.ReadBuildInfo(); ok {
		for _, dep := range info.Deps {
			if dep.Path == "go.podman.io/buildah" {
				return dep.Version
			}
		}
	}
	return buildah.Version
}

// hostSupportedPlatforms reports the platforms this sidecar can build for
// without binfmt cross-emulation. The host's GOOS/GOARCH is always
// present; if QEMU binfmt is registered, callers can detect that
// separately via a future RPC.
func hostSupportedPlatforms() []string {
	return []string{runtime.GOOS + "/" + runtime.GOARCH}
}

// resolveSidecarVersion reads the main module's version from build info.
// This is duplicated from main.go on purpose: ops.go is in a different
// package and we want this RPC to function without leaking main's symbols.
func resolveSidecarVersion() string {
	if info, ok := debug.ReadBuildInfo(); ok && info.Main.Version != "" {
		return info.Main.Version
	}
	return "(unknown)"
}
