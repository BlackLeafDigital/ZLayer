package server

import (
	"context"
	"errors"
	"fmt"
	"strings"
	"time"

	"github.com/google/uuid"
	pb "github.com/zorpxinc/zlayer/zlayer-buildd/internal/pb"
	"go.podman.io/buildah/define"
	"go.podman.io/buildah/imagebuildah"
	"go.podman.io/image/v5/docker/reference"
)

// Build implements pb.BuildServiceServer.Build. It translates a
// pb.BuildRequest into a define.BuildOptions, drives
// imagebuildah.BuildDockerfiles, and streams progress to the client as a
// sequence of pb.BuildEvent messages. The final event is always either a
// BuildFinished (success) or a BuildError (any failure), so clients can
// rely on stream termination for completion signalling.
func (s *Server) Build(req *pb.BuildRequest, stream pb.BuildService_BuildServer) error {
	s.touchActivity()
	defer s.touchActivity()

	// Concurrency gate. Drop the slot when this build exits.
	select {
	case s.sem <- struct{}{}:
		defer func() { <-s.sem }()
	case <-stream.Context().Done():
		return stream.Context().Err()
	}

	// Resolve or mint a request_id and register cancellation.
	reqID := strings.TrimSpace(req.GetRequestId())
	if reqID == "" {
		reqID = uuid.NewString()
	}

	ctx, cancel := context.WithCancel(stream.Context())
	defer cancel()
	if err := s.registerInflight(reqID, cancel); err != nil {
		return sendBuildError(stream, "register", err)
	}
	defer s.unregisterInflight(reqID)

	// Identify dockerfile paths up-front so we fail fast with a clear
	// error before any expensive option translation. Parse errors take
	// precedence over environmental errors so a malformed request always
	// surfaces the same way regardless of daemon state.
	paths := req.GetDockerfilePaths()
	if len(paths) == 0 {
		return sendBuildError(stream, "parse", errors.New("no dockerfile_paths provided in BuildRequest"))
	}

	// Sidecar holds one store for the lifetime of the daemon.
	store := s.opts.Store
	if store == nil {
		return sendBuildError(stream, "init", errors.New("zlayer-buildd was started without a containers/storage store"))
	}

	// Wire stdout/stderr into per-line LogLine events. Flush at the end so
	// the trailing image-id line (which buildah typically prints without a
	// terminating newline) is preserved.
	outWriter := newEventWriter(stream, false)
	errWriter := newEventWriter(stream, true)
	defer outWriter.Flush()
	defer errWriter.Flush()

	// Translate proto → define.BuildOptions, attaching the systemContext
	// from server options.
	options, warnings, perr := buildOptionsFromRequest(req, outWriter, errWriter, stream)
	if perr != nil {
		return sendBuildError(stream, "parse", perr)
	}
	options.SystemContext = s.opts.SystemContext

	// Surface any "no clean buildah home for this field" warnings before
	// kicking off the build so clients always see them.
	for _, msg := range warnings {
		if err := stream.Send(&pb.BuildEvent{
			Event: &pb.BuildEvent_Warning{Warning: &pb.Warning{Message: msg}},
		}); err != nil {
			return err
		}
	}

	imageID, ref, err := imagebuildah.BuildDockerfiles(ctx, store, options, paths...)
	if err != nil {
		// Distinguish cancellation from a real build failure so clients
		// can render it differently.
		kind := "exec"
		if errors.Is(err, context.Canceled) || errors.Is(ctx.Err(), context.Canceled) {
			kind = "cancelled"
		} else if errors.Is(err, context.DeadlineExceeded) || errors.Is(ctx.Err(), context.DeadlineExceeded) {
			kind = "deadline"
		}
		return sendBuildError(stream, kind, err)
	}

	finishedRef := ""
	if ref != nil {
		finishedRef = ref.String()
	}
	return stream.Send(&pb.BuildEvent{
		Event: &pb.BuildEvent_Finished{
			Finished: &pb.BuildFinished{
				ImageId:     imageID,
				ManifestRef: finishedRef,
			},
		},
	})
}

// Cancel implements pb.BuildServiceServer.Cancel. Returns Cancelled=true if
// a matching in-flight build was found and signalled. Cancelled=false when
// no such request_id is currently registered (already finished, or never
// existed).
func (s *Server) Cancel(_ context.Context, req *pb.CancelRequest) (*pb.CancelResponse, error) {
	s.touchActivity()
	defer s.touchActivity()
	reqID := strings.TrimSpace(req.GetRequestId())
	if reqID == "" {
		return &pb.CancelResponse{Cancelled: false}, nil
	}
	return &pb.CancelResponse{Cancelled: s.cancelInflight(reqID)}, nil
}

// sendBuildError sends a single BuildEvent_Error and returns nil. We
// deliberately return nil rather than the original error: BuildError is the
// in-band failure signal, and surfacing the same error via the gRPC status
// would cause clients to double-report. If the stream itself is broken the
// Send call returns an error which we propagate so the gRPC layer can clean
// up.
func sendBuildError(stream pb.BuildService_BuildServer, kind string, err error) error {
	sendErr := stream.Send(&pb.BuildEvent{
		Event: &pb.BuildEvent_Error{
			Error: &pb.BuildError{Message: err.Error(), Kind: kind},
		},
	})
	if sendErr != nil {
		return sendErr
	}
	return nil
}

// buildOptionsFromRequest translates a pb.BuildRequest into a populated
// define.BuildOptions. It returns a list of human-readable warnings for
// fields the schema exposes that have no clean buildah analog at v1.44.0
// (those are surfaced to the client as Warning events) and an error for
// anything genuinely malformed (bad platform string, unknown pull policy,
// etc.).
//
// The unused `stream` parameter is reserved so future translators can emit
// progress events while parsing without rewiring the signature.
func buildOptionsFromRequest(
	req *pb.BuildRequest,
	out, errW *eventWriter,
	_ pb.BuildService_BuildServer,
) (define.BuildOptions, []string, error) {
	if req == nil {
		return define.BuildOptions{}, nil, errors.New("nil BuildRequest")
	}

	var warnings []string

	// Tags: first → Output, rest → AdditionalTags.
	var output string
	var additionalTags []string
	if len(req.GetTags()) > 0 {
		output = req.GetTags()[0]
		if len(req.GetTags()) > 1 {
			additionalTags = append(additionalTags, req.GetTags()[1:]...)
		}
	}

	// Output manifest format.
	outputFormat, err := parseOutputFormat(req.GetFormat())
	if err != nil {
		return define.BuildOptions{}, nil, err
	}

	// Pull policy.
	pullPolicy, err := parsePullPolicy(req.GetPullPolicy())
	if err != nil {
		return define.BuildOptions{}, nil, err
	}

	// Platforms.
	platforms, err := parsePlatforms(req.GetPlatforms())
	if err != nil {
		return define.BuildOptions{}, nil, err
	}

	// Cache references.
	cacheFrom, err := parseCacheRefs("cache_from", req.GetCacheFrom())
	if err != nil {
		return define.BuildOptions{}, nil, err
	}
	cacheTo, err := parseCacheRefs("cache_to", req.GetCacheTo())
	if err != nil {
		return define.BuildOptions{}, nil, err
	}

	// SourceDateEpoch: 0 means unset.
	var sde *time.Time
	if epoch := req.GetSourceDateEpoch(); epoch != 0 {
		t := time.Unix(epoch, 0).UTC()
		sde = &t
	}

	// CommonBuildOpts MUST be non-nil per buildah's contract. Even when
	// every field below is the zero value, buildah panics if this pointer
	// is nil.
	common := &define.CommonBuildOptions{
		AddHost:    append([]string(nil), req.GetAddHosts()...),
		ShmSize:    req.GetShmSize(),
		Ulimit:     append([]string(nil), req.GetUlimits()...),
		Volumes:    append([]string(nil), req.GetVolumes()...),
		Secrets:    append([]string(nil), req.GetSecrets()...),
		SSHSources: append([]string(nil), req.GetSsh()...),
	}

	// Network policy. Schema only exposes "use host networking" as a
	// boolean; everything else (NetworkDefault, NetworkDisabled) stays at
	// the buildah default. NetworkEnabled is the closest semantic match
	// for --network=host within buildah's three-valued enum: it tells
	// buildah to wire up the build container's network, which combined
	// with the host's namespace yields host networking. The actual
	// namespace choice lives in NamespaceOptions, set below.
	configureNetwork := define.NetworkDefault
	var namespaceOptions []define.NamespaceOption
	if req.GetHostNetwork() {
		configureNetwork = define.NetworkEnabled
		namespaceOptions = append(namespaceOptions, define.NamespaceOption{
			Name: "network",
			Host: true,
		})
	}

	// Cache-from/cache-to are translated into reference.Named lists, but
	// the schema only carries one string for each side today. If we
	// receive a comma-separated list we expand it; otherwise it is a
	// single entry. Empty string short-circuits.
	options := define.BuildOptions{
		ContextDirectory: req.GetContextDir(),
		PullPolicy:       pullPolicy,
		Args:             req.GetBuildArgs(),
		Output:           output,
		AdditionalTags:   additionalTags,
		Out:              out,
		Err:              errW,
		ReportWriter:     errW,
		Log: func(format string, args ...any) {
			// buildah's Log callback is line-oriented; route every
			// message as a stderr LogLine event. Funnel through the
			// errW writer so timing is consistent with anything
			// buildah writes directly to Err.
			_, _ = fmt.Fprintf(errW, format, args...)
			// buildah's contract: Log messages may or may not end in
			// '\n'. Force a newline so eventWriter flushes the line
			// promptly instead of waiting for the next write.
			if !strings.HasSuffix(fmt.Sprintf(format, args...), "\n") {
				_, _ = errW.Write([]byte{'\n'})
			}
		},
		OutputFormat:     outputFormat,
		CommonBuildOpts:  common,
		Squash:           req.GetSquash(),
		Layers:           req.GetLayers(),
		NoCache:          req.GetNoCache(),
		Labels:           append([]string(nil), req.GetLabels()...),
		Annotations:      append([]string(nil), req.GetAnnotations()...),
		Target:           req.GetTargetStage(),
		Envs:             append([]string(nil), req.GetEnvs()...),
		Platforms:        platforms,
		SourceDateEpoch:  sde,
		RewriteTimestamp: req.GetRewriteTimestamp(),
		CacheFrom:        cacheFrom,
		CacheTo:          cacheTo,
		ConfigureNetwork: configureNetwork,
		NamespaceOptions: namespaceOptions,
	}

	// Surface fields that have no buildah analog. Today every schema
	// field maps to *something*, but we keep this scaffold so adding a
	// new proto field that we cannot map shows up as a Warning event
	// rather than silent data loss.
	_ = warnings // kept addressable; appended above if/when needed.

	return options, warnings, nil
}

// parseOutputFormat maps the proto "format" string to buildah's manifest
// MIME type constant. Empty string defaults to OCI to match the rest of
// ZLayer's image conventions.
func parseOutputFormat(format string) (string, error) {
	switch strings.ToLower(strings.TrimSpace(format)) {
	case "", "oci":
		return define.OCIv1ImageManifest, nil
	case "docker":
		return define.Dockerv2ImageManifest, nil
	default:
		return "", fmt.Errorf("unsupported format %q (allowed: oci, docker)", format)
	}
}

// parsePullPolicy maps the proto "pull_policy" string to buildah's enum.
// Empty defaults to PullIfMissing (buildah's own default).
func parsePullPolicy(s string) (define.PullPolicy, error) {
	switch strings.ToLower(strings.TrimSpace(s)) {
	case "":
		return define.PullIfMissing, nil
	case "always":
		return define.PullAlways, nil
	case "missing", "ifmissing", "if-missing":
		return define.PullIfMissing, nil
	case "never":
		return define.PullNever, nil
	case "ifnewer", "if-newer", "newer":
		return define.PullIfNewer, nil
	default:
		return 0, fmt.Errorf("unsupported pull_policy %q (allowed: always, missing, never, ifnewer)", s)
	}
}

// parsePlatforms parses a list of "os/arch[/variant]" strings into the
// struct slice that define.BuildOptions.Platforms expects. Empty slice ⇒
// nil (single-platform build using BuildOptions.OS / .Architecture).
func parsePlatforms(specs []string) ([]struct{ OS, Arch, Variant string }, error) {
	if len(specs) == 0 {
		return nil, nil
	}
	out := make([]struct{ OS, Arch, Variant string }, 0, len(specs))
	for _, s := range specs {
		s = strings.TrimSpace(s)
		if s == "" {
			continue
		}
		parts := strings.Split(s, "/")
		var p struct{ OS, Arch, Variant string }
		switch len(parts) {
		case 2:
			p.OS = strings.TrimSpace(parts[0])
			p.Arch = strings.TrimSpace(parts[1])
		case 3:
			p.OS = strings.TrimSpace(parts[0])
			p.Arch = strings.TrimSpace(parts[1])
			p.Variant = strings.TrimSpace(parts[2])
		default:
			return nil, fmt.Errorf("invalid platform %q (want os/arch or os/arch/variant)", s)
		}
		if p.OS == "" || p.Arch == "" {
			return nil, fmt.Errorf("invalid platform %q (os and arch are required)", s)
		}
		out = append(out, p)
	}
	if len(out) == 0 {
		return nil, nil
	}
	return out, nil
}

// parseCacheRefs parses a single proto "cache_from" / "cache_to" string
// into a []reference.Named slice. We accept comma-separated entries to
// match buildah's CLI convention even though the schema is one string.
// Empty input returns a nil slice (buildah disables caching for that side).
func parseCacheRefs(field, raw string) ([]reference.Named, error) {
	raw = strings.TrimSpace(raw)
	if raw == "" {
		return nil, nil
	}
	pieces := strings.Split(raw, ",")
	out := make([]reference.Named, 0, len(pieces))
	for _, p := range pieces {
		p = strings.TrimSpace(p)
		if p == "" {
			continue
		}
		ref, err := reference.ParseNormalizedNamed(p)
		if err != nil {
			return nil, fmt.Errorf("invalid %s reference %q: %w", field, p, err)
		}
		out = append(out, ref)
	}
	if len(out) == 0 {
		return nil, nil
	}
	return out, nil
}
