package server

import (
	"context"
	"fmt"
	"sync"
	"sync/atomic"

	pb "github.com/zorpxinc/zlayer/zlayer-buildd/internal/pb"
	"go.podman.io/image/v5/types"
	"go.podman.io/storage"
)

// Options configures a Server.
type Options struct {
	// Store is the containers/storage store buildah uses to materialise
	// layers. Constructed by main() via storage.GetStore(...) so the same
	// store can be shared with future RPCs (Inspect, etc.).
	Store storage.Store

	// MaxConcurrent caps the number of simultaneous Build streams. Each
	// build holds significant memory + disk, so we serialise by default.
	// Values <= 0 are clamped to 1.
	MaxConcurrent int

	// SystemContext is buildah's pull/auth credential bundle, forwarded
	// straight into define.BuildOptions.SystemContext for every Build.
	// May be nil (buildah uses package defaults).
	SystemContext *types.SystemContext
}

// Server implements pb.BuildServiceServer. Holds the buildah store and a
// registry of in-flight builds for the Cancel RPC.
type Server struct {
	pb.UnimplementedBuildServiceServer

	opts Options

	// inflight maps request_id → cancel func so Cancel can unwind any
	// running build cooperatively.
	inflightMu sync.Mutex
	inflight   map[string]context.CancelFunc

	// lastActivityNs records the time of the last RPC boundary (start or
	// end). main() reads it for the idle-shutdown timer.
	lastActivityNs atomic.Int64

	// sem caps concurrent Build streams.
	sem chan struct{}
}

// New constructs a Server. After New, register it with a *grpc.Server via
// pb.RegisterBuildServiceServer.
func New(opts Options) *Server {
	if opts.MaxConcurrent <= 0 {
		opts.MaxConcurrent = 1
	}
	s := &Server{
		opts:     opts,
		inflight: make(map[string]context.CancelFunc),
		sem:      make(chan struct{}, opts.MaxConcurrent),
	}
	s.touchActivity()
	return s
}

// touchActivity records that an RPC just started or ended. main() polls
// LastActivityUnixNanos() to drive idle shutdown.
func (s *Server) touchActivity() {
	s.lastActivityNs.Store(nowUnixNanos())
}

// LastActivityUnixNanos returns the wall-clock time of the most recent RPC
// boundary (start or end). Used by main()'s idle-shutdown loop.
func (s *Server) LastActivityUnixNanos() int64 {
	return s.lastActivityNs.Load()
}

// Store exposes the underlying containers/storage store for other RPCs
// (Inspect, Health) implemented in ops.go.
func (s *Server) Store() storage.Store {
	return s.opts.Store
}

// SystemContext returns the configured systemContext (may be nil). Exposed
// so future RPCs can reuse the same credentials bundle.
func (s *Server) SystemContext() *types.SystemContext {
	return s.opts.SystemContext
}

// registerInflight associates a request_id with its cancel func. Returns an
// error if the id is already in use.
func (s *Server) registerInflight(reqID string, cancel context.CancelFunc) error {
	s.inflightMu.Lock()
	defer s.inflightMu.Unlock()
	if _, exists := s.inflight[reqID]; exists {
		return fmt.Errorf("build request_id %q already in flight", reqID)
	}
	s.inflight[reqID] = cancel
	return nil
}

// unregisterInflight removes a request_id from the cancel registry.
func (s *Server) unregisterInflight(reqID string) {
	s.inflightMu.Lock()
	defer s.inflightMu.Unlock()
	delete(s.inflight, reqID)
}

// InflightCount returns the number of builds currently executing. main()'s
// idle-shutdown loop uses this to avoid killing the process during a
// long-running build that hasn't emitted activity in a while (e.g. waiting
// on a slow registry pull).
func (s *Server) InflightCount() int {
	s.inflightMu.Lock()
	defer s.inflightMu.Unlock()
	return len(s.inflight)
}

// cancelInflight signals the registered context-cancel for reqID, if any.
// Returns whether a build was found.
func (s *Server) cancelInflight(reqID string) bool {
	s.inflightMu.Lock()
	defer s.inflightMu.Unlock()
	cancel, ok := s.inflight[reqID]
	if !ok {
		return false
	}
	cancel()
	return true
}
