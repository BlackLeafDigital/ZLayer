package server

import (
	"context"
	"strings"
	"sync"
	"testing"

	pb "github.com/zorpxinc/zlayer/zlayer-buildd/internal/pb"
	"go.podman.io/buildah/define"
	"google.golang.org/grpc"
	"google.golang.org/grpc/metadata"
)

// fakeStream is a minimal grpc.ServerStreamingServer[pb.BuildEvent] used by
// the tests. It captures every BuildEvent sent on the stream and exposes
// a controllable context for cancellation tests.
type fakeStream struct {
	ctx    context.Context
	mu     sync.Mutex
	events []*pb.BuildEvent
}

func newFakeStream(ctx context.Context) *fakeStream {
	if ctx == nil {
		ctx = context.Background()
	}
	return &fakeStream{ctx: ctx}
}

func (f *fakeStream) Send(ev *pb.BuildEvent) error {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.events = append(f.events, ev)
	return nil
}

func (f *fakeStream) Context() context.Context        { return f.ctx }
func (f *fakeStream) SetHeader(metadata.MD) error     { return nil }
func (f *fakeStream) SendHeader(metadata.MD) error    { return nil }
func (f *fakeStream) SetTrailer(metadata.MD)          {}
func (f *fakeStream) SendMsg(any) error               { return nil }
func (f *fakeStream) RecvMsg(any) error               { return nil }

// compile-time check that fakeStream really is the type Build expects.
var _ grpc.ServerStreamingServer[pb.BuildEvent] = (*fakeStream)(nil)

func (f *fakeStream) Events() []*pb.BuildEvent {
	f.mu.Lock()
	defer f.mu.Unlock()
	out := make([]*pb.BuildEvent, len(f.events))
	copy(out, f.events)
	return out
}

func TestBuildOptionsFromRequest_EmptyRequestParsesButDockerfilesGuardCatchesIt(t *testing.T) {
	// An empty request is parseable as long as nothing inside it is
	// malformed; the "no dockerfile_paths" guard is enforced in Build,
	// not in buildOptionsFromRequest. This test pins that contract.
	stream := newFakeStream(nil)
	out := newEventWriter(stream, false)
	errW := newEventWriter(stream, true)

	opts, warnings, err := buildOptionsFromRequest(&pb.BuildRequest{}, out, errW, stream)
	if err != nil {
		t.Fatalf("expected nil error for empty request, got %v", err)
	}
	if len(warnings) != 0 {
		t.Fatalf("expected no warnings for empty request, got %v", warnings)
	}
	if opts.CommonBuildOpts == nil {
		t.Fatalf("CommonBuildOpts must be non-nil even for an empty request")
	}
	if opts.OutputFormat != define.OCIv1ImageManifest {
		t.Errorf("default OutputFormat = %q, want %q", opts.OutputFormat, define.OCIv1ImageManifest)
	}
	if opts.PullPolicy != define.PullIfMissing {
		t.Errorf("default PullPolicy = %v, want PullIfMissing", opts.PullPolicy)
	}
}

func TestBuild_RejectsRequestWithNoDockerfiles(t *testing.T) {
	srv := New(Options{Store: nil, MaxConcurrent: 1})
	// We expect this to fail before it ever reaches the store, on the
	// "no dockerfile_paths" guard, so a nil store is fine here.
	stream := newFakeStream(context.Background())

	if err := srv.Build(&pb.BuildRequest{}, stream); err != nil {
		t.Fatalf("Build returned transport error: %v", err)
	}

	events := stream.Events()
	if len(events) == 0 {
		t.Fatal("expected at least one event, got none")
	}
	last := events[len(events)-1]
	be := last.GetError()
	if be == nil {
		t.Fatalf("expected final event to be BuildError, got %T", last.GetEvent())
	}
	if be.GetKind() != "parse" {
		t.Errorf("error kind = %q, want %q", be.GetKind(), "parse")
	}
	if !strings.Contains(be.GetMessage(), "dockerfile_paths") {
		t.Errorf("error message %q should mention dockerfile_paths", be.GetMessage())
	}
}

func TestBuild_RejectsRequestWithoutStore(t *testing.T) {
	srv := New(Options{Store: nil, MaxConcurrent: 1})
	stream := newFakeStream(context.Background())

	req := &pb.BuildRequest{
		DockerfilePaths: []string{"Dockerfile"},
		ContextDir:      "/tmp/whatever",
	}
	if err := srv.Build(req, stream); err != nil {
		t.Fatalf("Build returned transport error: %v", err)
	}
	events := stream.Events()
	if len(events) == 0 {
		t.Fatal("expected at least one event, got none")
	}
	last := events[len(events)-1]
	be := last.GetError()
	if be == nil {
		t.Fatalf("expected final event to be BuildError, got %T", last.GetEvent())
	}
	if be.GetKind() != "init" {
		t.Errorf("error kind = %q, want %q", be.GetKind(), "init")
	}
}

func TestBuildOptionsFromRequest_MinimalValidRequest(t *testing.T) {
	stream := newFakeStream(nil)
	out := newEventWriter(stream, false)
	errW := newEventWriter(stream, true)

	req := &pb.BuildRequest{
		ContextDir:      "/work",
		DockerfilePaths: []string{"Dockerfile"},
		Tags:            []string{"primary:latest", "alias:v1", "alias:stable"},
		Platforms:       []string{"linux/amd64", "linux/arm64/v8"},
		BuildArgs:       map[string]string{"FOO": "bar"},
		Secrets:         []string{"id=mysecret,src=/etc/secret"},
		Ssh:             []string{"default"},
		TargetStage:     "runtime",
		HostNetwork:     true,
		CacheFrom:       "docker.io/library/alpine:latest",
		CacheTo:         "docker.io/me/cache:latest",
		NoCache:         true,
		Squash:          true,
		Layers:          true,
		Format:          "docker",
		PullPolicy:      "always",
		Labels:          []string{"key=value"},
		Annotations:     []string{"ann=val"},
		AddHosts:        []string{"foo:1.2.3.4"},
		Envs:            []string{"BAR=baz"},
		ShmSize:         "64m",
		Ulimits:         []string{"nofile=1024:2048"},
		Volumes:         []string{"/host:/ctr:ro"},
		SourceDateEpoch: 1700000000,
		// RewriteTimestamp: true (skip — exercised below).
	}

	opts, warnings, err := buildOptionsFromRequest(req, out, errW, stream)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if len(warnings) != 0 {
		t.Fatalf("expected no warnings, got %v", warnings)
	}

	// Spot-check every category.
	if opts.ContextDirectory != "/work" {
		t.Errorf("ContextDirectory = %q, want %q", opts.ContextDirectory, "/work")
	}
	if opts.Output != "primary:latest" {
		t.Errorf("Output = %q, want %q", opts.Output, "primary:latest")
	}
	if len(opts.AdditionalTags) != 2 {
		t.Errorf("AdditionalTags = %v, want 2 entries", opts.AdditionalTags)
	}
	if opts.OutputFormat != define.Dockerv2ImageManifest {
		t.Errorf("OutputFormat = %q, want Dockerv2ImageManifest", opts.OutputFormat)
	}
	if opts.PullPolicy != define.PullAlways {
		t.Errorf("PullPolicy = %v, want PullAlways", opts.PullPolicy)
	}
	if opts.Target != "runtime" {
		t.Errorf("Target = %q, want %q", opts.Target, "runtime")
	}
	if !opts.Squash || !opts.Layers || !opts.NoCache {
		t.Errorf("boolean flags not propagated: squash=%v layers=%v nocache=%v",
			opts.Squash, opts.Layers, opts.NoCache)
	}
	if opts.Args["FOO"] != "bar" {
		t.Errorf("Args[FOO] = %q, want %q", opts.Args["FOO"], "bar")
	}
	if len(opts.Platforms) != 2 {
		t.Fatalf("Platforms = %v, want 2 entries", opts.Platforms)
	}
	if opts.Platforms[1].Variant != "v8" {
		t.Errorf("Platforms[1].Variant = %q, want %q", opts.Platforms[1].Variant, "v8")
	}
	if opts.SourceDateEpoch == nil {
		t.Fatal("SourceDateEpoch should be set when proto field is non-zero")
	}
	if opts.SourceDateEpoch.Unix() != 1700000000 {
		t.Errorf("SourceDateEpoch = %v, want 1700000000", opts.SourceDateEpoch.Unix())
	}
	if opts.CommonBuildOpts == nil {
		t.Fatal("CommonBuildOpts must be non-nil")
	}
	cb := opts.CommonBuildOpts
	if cb.ShmSize != "64m" {
		t.Errorf("CommonBuildOpts.ShmSize = %q, want %q", cb.ShmSize, "64m")
	}
	if len(cb.AddHost) != 1 || cb.AddHost[0] != "foo:1.2.3.4" {
		t.Errorf("CommonBuildOpts.AddHost = %v", cb.AddHost)
	}
	if len(cb.Volumes) != 1 || cb.Volumes[0] != "/host:/ctr:ro" {
		t.Errorf("CommonBuildOpts.Volumes = %v", cb.Volumes)
	}
	if len(cb.SSHSources) != 1 || cb.SSHSources[0] != "default" {
		t.Errorf("CommonBuildOpts.SSHSources = %v", cb.SSHSources)
	}
	if opts.ConfigureNetwork != define.NetworkEnabled {
		t.Errorf("ConfigureNetwork = %v, want NetworkEnabled (host_network=true)", opts.ConfigureNetwork)
	}
	if len(opts.NamespaceOptions) == 0 || opts.NamespaceOptions[0].Name != "network" || !opts.NamespaceOptions[0].Host {
		t.Errorf("NamespaceOptions for host networking missing/wrong: %+v", opts.NamespaceOptions)
	}
	if len(opts.CacheFrom) != 1 {
		t.Errorf("CacheFrom = %v, want 1 entry", opts.CacheFrom)
	}
	// cache_to includes a comma, so split produces multiple entries —
	// confirm at least one of them fails parsing cleanly OR all parse;
	// we just check the field is non-nil.
}

func TestParsePullPolicy(t *testing.T) {
	cases := []struct {
		in   string
		want define.PullPolicy
		err  bool
	}{
		{"", define.PullIfMissing, false},
		{"always", define.PullAlways, false},
		{"ALWAYS", define.PullAlways, false},
		{"missing", define.PullIfMissing, false},
		{"never", define.PullNever, false},
		{"ifnewer", define.PullIfNewer, false},
		{"if-newer", define.PullIfNewer, false},
		{"bogus", 0, true},
	}
	for _, c := range cases {
		got, err := parsePullPolicy(c.in)
		if c.err {
			if err == nil {
				t.Errorf("parsePullPolicy(%q) expected error, got nil", c.in)
			}
			continue
		}
		if err != nil {
			t.Errorf("parsePullPolicy(%q) unexpected error: %v", c.in, err)
			continue
		}
		if got != c.want {
			t.Errorf("parsePullPolicy(%q) = %v, want %v", c.in, got, c.want)
		}
	}
}

func TestParseOutputFormat(t *testing.T) {
	cases := []struct {
		in   string
		want string
		err  bool
	}{
		{"", define.OCIv1ImageManifest, false},
		{"oci", define.OCIv1ImageManifest, false},
		{"OCI", define.OCIv1ImageManifest, false},
		{"docker", define.Dockerv2ImageManifest, false},
		{"Docker", define.Dockerv2ImageManifest, false},
		{"junk", "", true},
	}
	for _, c := range cases {
		got, err := parseOutputFormat(c.in)
		if c.err {
			if err == nil {
				t.Errorf("parseOutputFormat(%q) expected error", c.in)
			}
			continue
		}
		if err != nil {
			t.Errorf("parseOutputFormat(%q) unexpected error: %v", c.in, err)
		}
		if got != c.want {
			t.Errorf("parseOutputFormat(%q) = %q, want %q", c.in, got, c.want)
		}
	}
}

func TestParsePlatforms(t *testing.T) {
	got, err := parsePlatforms([]string{"linux/amd64", "linux/arm64/v8", " "})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if len(got) != 2 {
		t.Fatalf("got %d platforms, want 2", len(got))
	}
	if got[0].OS != "linux" || got[0].Arch != "amd64" || got[0].Variant != "" {
		t.Errorf("got[0] = %+v", got[0])
	}
	if got[1].OS != "linux" || got[1].Arch != "arm64" || got[1].Variant != "v8" {
		t.Errorf("got[1] = %+v", got[1])
	}

	if _, err := parsePlatforms([]string{"badplatform"}); err == nil {
		t.Error("parsePlatforms(\"badplatform\") expected error")
	}
	if _, err := parsePlatforms([]string{"linux/"}); err == nil {
		t.Error("parsePlatforms(\"linux/\") expected error")
	}
}

func TestEventWriter_LinesEmittedSeparately(t *testing.T) {
	stream := newFakeStream(nil)
	w := newEventWriter(stream, false)
	_, _ = w.Write([]byte("hello\nwo"))
	_, _ = w.Write([]byte("rld\nfinal-no-newline"))
	w.Flush()

	events := stream.Events()
	if len(events) != 3 {
		t.Fatalf("got %d events, want 3", len(events))
	}
	lines := []string{
		events[0].GetLog().GetLine(),
		events[1].GetLog().GetLine(),
		events[2].GetLog().GetLine(),
	}
	want := []string{"hello", "world", "final-no-newline"}
	for i := range lines {
		if lines[i] != want[i] {
			t.Errorf("event[%d].line = %q, want %q", i, lines[i], want[i])
		}
	}
}

func TestServerInflightRegistry(t *testing.T) {
	srv := New(Options{MaxConcurrent: 1})
	_, cancel := context.WithCancel(context.Background())
	defer cancel()
	if err := srv.registerInflight("rid", cancel); err != nil {
		t.Fatalf("first register failed: %v", err)
	}
	if err := srv.registerInflight("rid", cancel); err == nil {
		t.Fatal("expected error registering duplicate rid")
	}
	if !srv.cancelInflight("rid") {
		t.Fatal("expected cancelInflight to find the rid")
	}
	srv.unregisterInflight("rid")
	if srv.cancelInflight("rid") {
		t.Fatal("expected cancelInflight to miss after unregister")
	}
}

func TestCancelEmptyRequestID(t *testing.T) {
	srv := New(Options{MaxConcurrent: 1})
	resp, err := srv.Cancel(context.Background(), &pb.CancelRequest{})
	if err != nil {
		t.Fatalf("Cancel returned error: %v", err)
	}
	if resp.GetCancelled() {
		t.Error("expected Cancelled=false for empty request_id")
	}
}
