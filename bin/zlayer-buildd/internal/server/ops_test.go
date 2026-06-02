package server

import (
	"context"
	"testing"

	pb "github.com/zorpxinc/zlayer/zlayer-buildd/internal/pb"
	"go.podman.io/storage"
	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/status"
)

func TestHealth_ReturnsNotServingWhenStoreNil(t *testing.T) {
	srv := New(Options{Store: nil})
	resp, err := srv.Health(context.Background(), &pb.HealthRequest{})
	if err != nil {
		t.Fatalf("Health returned error: %v", err)
	}
	if resp.Status != pb.HealthResponse_NOT_SERVING {
		t.Fatalf("expected NOT_SERVING, got %v", resp.Status)
	}
	if resp.Detail == "" {
		t.Fatalf("expected non-empty detail when not serving")
	}
}

func TestInspect_PopulatesPlatforms(t *testing.T) {
	srv := New(Options{Store: nil})
	resp, err := srv.Inspect(context.Background(), &pb.InspectRequest{})
	if err != nil {
		t.Fatalf("Inspect returned error: %v", err)
	}
	if len(resp.SupportedPlatforms) == 0 {
		t.Fatalf("expected at least one supported platform")
	}
	if resp.SidecarVersion == "" {
		t.Fatalf("expected sidecar version to be non-empty (got %q)", resp.SidecarVersion)
	}
}

func TestInspect_OmitsStorageWhenStoreNil(t *testing.T) {
	srv := New(Options{Store: nil})
	resp, err := srv.Inspect(context.Background(), &pb.InspectRequest{})
	if err != nil {
		t.Fatalf("Inspect returned error: %v", err)
	}
	if resp.StorageDriver != "" {
		t.Fatalf("expected empty StorageDriver when store is nil, got %q", resp.StorageDriver)
	}
	if resp.StorageRoot != "" {
		t.Fatalf("expected empty StorageRoot when store is nil, got %q", resp.StorageRoot)
	}
	if resp.BuildahVersion == "" {
		t.Fatalf("expected non-empty BuildahVersion")
	}
}

// expectGRPCCode asserts that err is a gRPC status error with the given
// code. Used by the validation tests below so each failure case asserts
// both that something went wrong AND that it went wrong for the right
// reason (FailedPrecondition vs InvalidArgument is the canonical
// difference clients care about).
func expectGRPCCode(t *testing.T, err error, want codes.Code) {
	t.Helper()
	if err == nil {
		t.Fatalf("expected gRPC error with code %s, got nil", want)
	}
	st, ok := status.FromError(err)
	if !ok {
		t.Fatalf("expected gRPC status error, got %T: %v", err, err)
	}
	if st.Code() != want {
		t.Fatalf("expected code %s, got %s (message=%q)", want, st.Code(), st.Message())
	}
}

func TestPush_FailsFastWithoutStore(t *testing.T) {
	srv := New(Options{Store: nil})
	_, err := srv.Push(context.Background(), &pb.PushRequest{
		Image:       "foo:latest",
		Destination: "registry.example.com/foo:latest",
	})
	expectGRPCCode(t, err, codes.FailedPrecondition)
}

func TestTag_FailsFastWithoutStore(t *testing.T) {
	srv := New(Options{Store: nil})
	_, err := srv.Tag(context.Background(), &pb.TagRequest{
		Image:  "foo:latest",
		NewTag: "foo:v1",
	})
	expectGRPCCode(t, err, codes.FailedPrecondition)
}

func TestManifestCreate_FailsFastWithoutStore(t *testing.T) {
	srv := New(Options{Store: nil})
	_, err := srv.ManifestCreate(context.Background(), &pb.ManifestCreateRequest{Name: "list"})
	expectGRPCCode(t, err, codes.FailedPrecondition)
}

func TestManifestAdd_FailsFastWithoutStore(t *testing.T) {
	srv := New(Options{Store: nil})
	_, err := srv.ManifestAdd(context.Background(), &pb.ManifestAddRequest{
		Manifest: "list",
		Image:    "foo:latest",
	})
	expectGRPCCode(t, err, codes.FailedPrecondition)
}

func TestManifestPush_FailsFastWithoutStore(t *testing.T) {
	srv := New(Options{Store: nil})
	_, err := srv.ManifestPush(context.Background(), &pb.ManifestPushRequest{
		Name:        "list",
		Destination: "registry.example.com/list:latest",
	})
	expectGRPCCode(t, err, codes.FailedPrecondition)
}

// fakeStore is a non-nil, never-called storage.Store stand-in. It exists
// only so we can drive the post-precondition validation paths in the
// RPCs (empty image, empty destination, bad format) without standing up
// a real containers/storage instance. Embedding the interface gives us
// every method as a nil-pointer call that panics, but every validation
// path under test must return before any such call happens — so a
// panic during these tests is the bug we're guarding against, not the
// happy path.
type fakeStore struct{ storage.Store }

// optsWithFakeStore returns an Options containing a non-nil store. The
// store will panic if any method is invoked — which is exactly what we
// want: the validation paths must reject the request before touching
// the store.
func optsWithFakeStore() Options {
	return Options{Store: fakeStore{}}
}

func TestPush_RejectsEmptyImage(t *testing.T) {
	srv := New(optsWithFakeStore())
	_, err := srv.Push(context.Background(), &pb.PushRequest{
		Image:       "",
		Destination: "registry.example.com/foo:latest",
	})
	expectGRPCCode(t, err, codes.InvalidArgument)
}

func TestPush_RejectsEmptyDestination(t *testing.T) {
	srv := New(optsWithFakeStore())
	_, err := srv.Push(context.Background(), &pb.PushRequest{
		Image:       "foo:latest",
		Destination: "",
	})
	expectGRPCCode(t, err, codes.InvalidArgument)
}

func TestPush_RejectsUnknownFormat(t *testing.T) {
	srv := New(optsWithFakeStore())
	_, err := srv.Push(context.Background(), &pb.PushRequest{
		Image:       "foo:latest",
		Destination: "registry.example.com/foo:latest",
		Format:      "bogus",
	})
	expectGRPCCode(t, err, codes.InvalidArgument)
}

func TestTag_RejectsEmptyImage(t *testing.T) {
	srv := New(optsWithFakeStore())
	_, err := srv.Tag(context.Background(), &pb.TagRequest{
		Image:  "",
		NewTag: "foo:v1",
	})
	expectGRPCCode(t, err, codes.InvalidArgument)
}

func TestTag_RejectsEmptyNewTag(t *testing.T) {
	srv := New(optsWithFakeStore())
	_, err := srv.Tag(context.Background(), &pb.TagRequest{
		Image:  "foo:latest",
		NewTag: "",
	})
	expectGRPCCode(t, err, codes.InvalidArgument)
}

func TestManifestCreate_RejectsEmptyName(t *testing.T) {
	srv := New(optsWithFakeStore())
	_, err := srv.ManifestCreate(context.Background(), &pb.ManifestCreateRequest{Name: ""})
	expectGRPCCode(t, err, codes.InvalidArgument)
}

func TestManifestAdd_RejectsEmptyManifest(t *testing.T) {
	srv := New(optsWithFakeStore())
	_, err := srv.ManifestAdd(context.Background(), &pb.ManifestAddRequest{
		Manifest: "",
		Image:    "foo:latest",
	})
	expectGRPCCode(t, err, codes.InvalidArgument)
}

func TestManifestAdd_RejectsEmptyImage(t *testing.T) {
	srv := New(optsWithFakeStore())
	_, err := srv.ManifestAdd(context.Background(), &pb.ManifestAddRequest{
		Manifest: "list",
		Image:    "",
	})
	expectGRPCCode(t, err, codes.InvalidArgument)
}

func TestManifestPush_RejectsEmptyName(t *testing.T) {
	srv := New(optsWithFakeStore())
	_, err := srv.ManifestPush(context.Background(), &pb.ManifestPushRequest{
		Name:        "",
		Destination: "registry.example.com/list:latest",
	})
	expectGRPCCode(t, err, codes.InvalidArgument)
}

func TestManifestPush_RejectsEmptyDestination(t *testing.T) {
	srv := New(optsWithFakeStore())
	_, err := srv.ManifestPush(context.Background(), &pb.ManifestPushRequest{
		Name:        "list",
		Destination: "",
	})
	expectGRPCCode(t, err, codes.InvalidArgument)
}
