package server

import (
	"context"
	"testing"

	pb "github.com/zorpxinc/zlayer/zlayer-buildd/internal/pb"
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
