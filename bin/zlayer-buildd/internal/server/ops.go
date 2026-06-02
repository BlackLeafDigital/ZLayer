package server

import (
	"context"
	"runtime"
	"runtime/debug"
	"strings"

	pb "github.com/zorpxinc/zlayer/zlayer-buildd/internal/pb"
	"go.podman.io/buildah"
	"go.podman.io/buildah/define"
	"go.podman.io/buildah/manifests"
	cp "go.podman.io/image/v5/copy"
	is "go.podman.io/image/v5/storage"
	"go.podman.io/image/v5/transports/alltransports"
	"go.podman.io/image/v5/types"
	"go.podman.io/storage"
	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/status"
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

// --- Push / Tag / Manifest -------------------------------------------------
//
// All five RPCs share the same precondition: the daemon must be running
// with a non-nil containers/storage store. Without it there is no
// image catalog to operate on, so we return FailedPrecondition.

// requireStore returns the live store or a FailedPrecondition error suitable
// for direct return from an RPC. Used by every post-build RPC.
func (s *Server) requireStore() error {
	if s.opts.Store == nil {
		return status.Error(codes.FailedPrecondition, "containers/storage store is not initialized")
	}
	return nil
}

// parsePushFormat maps the proto "format" field to a buildah manifest MIME
// type. Empty defaults to OCI to match buildOptionsFromRequest. Unknown
// values surface as InvalidArgument so the client gets a clear failure.
func parsePushFormat(format string) (string, error) {
	switch strings.ToLower(strings.TrimSpace(format)) {
	case "", "oci":
		return define.OCIv1ImageManifest, nil
	case "docker":
		return define.Dockerv2ImageManifest, nil
	default:
		return "", status.Errorf(codes.InvalidArgument, "unsupported format %q (allowed: oci, docker)", format)
	}
}

// systemContextWith returns a *types.SystemContext derived from the server's
// configured base (if any) with PushAuth overrides applied on top. The
// returned value is never nil so callers can pass it straight to buildah.
func (s *Server) systemContextWith(auth *pb.PushAuth) *types.SystemContext {
	var sys types.SystemContext
	if base := s.opts.SystemContext; base != nil {
		sys = *base
	}
	if auth == nil {
		return &sys
	}
	user := strings.TrimSpace(auth.GetUsername())
	pass := auth.GetPassword()
	identity := strings.TrimSpace(auth.GetIdentityToken())
	registry := strings.TrimSpace(auth.GetRegistryToken())
	if user == "" && pass == "" && identity == "" && registry == "" {
		return &sys
	}
	dac := &types.DockerAuthConfig{
		Username:      user,
		Password:      pass,
		IdentityToken: identity,
	}
	sys.DockerAuthConfig = dac
	if registry != "" {
		sys.DockerBearerRegistryToken = registry
	}
	return &sys
}

// parseDestRef converts a destination string into a types.ImageReference
// suitable for buildah.Push / list.Push. Accepts a transport-qualified ref
// (e.g. "docker://registry/foo:tag") or a bare ref ("registry/foo:tag"),
// canonicalising the bare form to docker://.
func parseDestRef(dest string) (types.ImageReference, error) {
	dest = strings.TrimSpace(dest)
	if dest == "" {
		return nil, status.Error(codes.InvalidArgument, "destination is required")
	}
	canonical := dest
	if !strings.Contains(canonical, "://") {
		canonical = "docker://" + canonical
	}
	ref, err := alltransports.ParseImageName(canonical)
	if err != nil {
		return nil, status.Errorf(codes.InvalidArgument, "invalid destination %q: %v", dest, err)
	}
	return ref, nil
}

// Push pushes an existing image in local containers/storage to a remote
// registry. The source image is looked up by local name or by `sha256:`
// digest; the destination is a registry reference (transport-prefixed or
// bare — bare refs default to `docker://`).
func (s *Server) Push(ctx context.Context, req *pb.PushRequest) (*pb.PushResponse, error) {
	s.touchActivity()
	defer s.touchActivity()

	if err := s.requireStore(); err != nil {
		return nil, err
	}
	image := strings.TrimSpace(req.GetImage())
	if image == "" {
		return nil, status.Error(codes.InvalidArgument, "image is required")
	}
	destRef, err := parseDestRef(req.GetDestination())
	if err != nil {
		return nil, err
	}
	manifestType, err := parsePushFormat(req.GetFormat())
	if err != nil {
		return nil, err
	}

	sys := s.systemContextWith(req.GetAuth())
	opts := buildah.PushOptions{
		Store:            s.opts.Store,
		SystemContext:    sys,
		ManifestType:     manifestType,
		RemoveSignatures: req.GetRemoveSignatures(),
		Quiet:            true,
	}

	_, dig, err := buildah.Push(ctx, image, destRef, opts)
	if err != nil {
		return nil, status.Errorf(codes.Internal, "buildah push %q: %v", image, err)
	}
	return &pb.PushResponse{PushedDigest: dig.String()}, nil
}

// Tag assigns an additional name to an existing image in local storage.
// Mirrors `buildah tag <image> <new_tag>`.
func (s *Server) Tag(_ context.Context, req *pb.TagRequest) (*pb.TagResponse, error) {
	s.touchActivity()
	defer s.touchActivity()

	if err := s.requireStore(); err != nil {
		return nil, err
	}
	image := strings.TrimSpace(req.GetImage())
	if image == "" {
		return nil, status.Error(codes.InvalidArgument, "image is required")
	}
	newTag := strings.TrimSpace(req.GetNewTag())
	if newTag == "" {
		return nil, status.Error(codes.InvalidArgument, "new_tag is required")
	}

	img, err := s.opts.Store.Image(image)
	if err != nil {
		return nil, status.Errorf(codes.NotFound, "image %q: %v", image, err)
	}
	if err := s.opts.Store.AddNames(img.ID, []string{newTag}); err != nil {
		return nil, status.Errorf(codes.Internal, "tag %q -> %q: %v", image, newTag, err)
	}
	return &pb.TagResponse{}, nil
}

// ManifestCreate creates a new (empty) manifest list and saves it under
// the supplied local name. Returns the resulting image ID.
func (s *Server) ManifestCreate(_ context.Context, req *pb.ManifestCreateRequest) (*pb.ManifestCreateResponse, error) {
	s.touchActivity()
	defer s.touchActivity()

	if err := s.requireStore(); err != nil {
		return nil, err
	}
	name := strings.TrimSpace(req.GetName())
	if name == "" {
		return nil, status.Error(codes.InvalidArgument, "name is required")
	}

	list := manifests.Create()
	// SaveToImage with imageID="" generates a fresh ID. MimeType OCIv1
	// matches the build-side default in buildOptionsFromRequest.
	id, err := list.SaveToImage(s.opts.Store, "", []string{name}, define.OCIv1ImageManifest)
	if err != nil {
		return nil, status.Errorf(codes.Internal, "manifest create %q: %v", name, err)
	}
	return &pb.ManifestCreateResponse{ListId: id}, nil
}

// ManifestAdd attaches an existing image to a manifest list. The image
// argument may be a local name, a `sha256:` digest, or a remote ref
// (alltransports-compatible) — bare refs default to the
// containers-storage transport so callers can name a freshly-built
// local image without ceremony.
func (s *Server) ManifestAdd(ctx context.Context, req *pb.ManifestAddRequest) (*pb.ManifestAddResponse, error) {
	s.touchActivity()
	defer s.touchActivity()

	if err := s.requireStore(); err != nil {
		return nil, err
	}
	manifestName := strings.TrimSpace(req.GetManifest())
	if manifestName == "" {
		return nil, status.Error(codes.InvalidArgument, "manifest is required")
	}
	image := strings.TrimSpace(req.GetImage())
	if image == "" {
		return nil, status.Error(codes.InvalidArgument, "image is required")
	}

	listID, list, err := manifests.LoadFromImage(s.opts.Store, manifestName)
	if err != nil {
		return nil, status.Errorf(codes.NotFound, "manifest %q: %v", manifestName, err)
	}

	imgRef, err := resolveImageRef(s.opts.Store, image)
	if err != nil {
		return nil, err
	}

	sys := s.systemContextWith(nil)
	dig, err := list.Add(ctx, sys, imgRef, true)
	if err != nil {
		return nil, status.Errorf(codes.Internal, "manifest add %q -> %q: %v", image, manifestName, err)
	}
	if _, err := list.SaveToImage(s.opts.Store, listID, nil, ""); err != nil {
		return nil, status.Errorf(codes.Internal, "manifest save %q: %v", manifestName, err)
	}
	return &pb.ManifestAddResponse{ImageDigest: dig.String()}, nil
}

// ManifestPush pushes a manifest list to a registry. When `all` is true,
// the referenced per-platform images are pushed alongside the list (this
// matches `buildah manifest push --all`, which is the only sensible
// default for multi-arch publishing).
func (s *Server) ManifestPush(ctx context.Context, req *pb.ManifestPushRequest) (*pb.ManifestPushResponse, error) {
	s.touchActivity()
	defer s.touchActivity()

	if err := s.requireStore(); err != nil {
		return nil, err
	}
	name := strings.TrimSpace(req.GetName())
	if name == "" {
		return nil, status.Error(codes.InvalidArgument, "name is required")
	}
	destRef, err := parseDestRef(req.GetDestination())
	if err != nil {
		return nil, err
	}

	_, list, err := manifests.LoadFromImage(s.opts.Store, name)
	if err != nil {
		return nil, status.Errorf(codes.NotFound, "manifest %q: %v", name, err)
	}

	selection := cp.CopySystemImage
	if req.GetAll() {
		selection = cp.CopyAllImages
	}
	sys := s.systemContextWith(req.GetAuth())
	opts := manifests.PushOptions{
		Store:              s.opts.Store,
		SystemContext:      sys,
		ImageListSelection: selection,
	}
	_, dig, err := list.Push(ctx, destRef, opts)
	if err != nil {
		return nil, status.Errorf(codes.Internal, "manifest push %q: %v", name, err)
	}
	return &pb.ManifestPushResponse{PushedDigest: dig.String()}, nil
}

// resolveImageRef converts an arbitrary image identifier into a
// types.ImageReference. Transport-prefixed names (e.g. "docker://...") go
// through alltransports; everything else is resolved through the
// containers-storage transport against the supplied store so a local name
// or sha256 ID just works.
func resolveImageRef(store storage.Store, image string) (types.ImageReference, error) {
	if strings.Contains(image, "://") {
		ref, err := alltransports.ParseImageName(image)
		if err != nil {
			return nil, status.Errorf(codes.InvalidArgument, "invalid image ref %q: %v", image, err)
		}
		return ref, nil
	}
	ref, err := is.Transport.ParseStoreReference(store, image)
	if err != nil {
		return nil, status.Errorf(codes.NotFound, "resolve %q in local storage: %v", image, err)
	}
	return ref, nil
}
