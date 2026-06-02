package server

import (
	"context"
	"testing"
)

func TestInflightCount_DefaultsZero(t *testing.T) {
	s := New(Options{})
	if got := s.InflightCount(); got != 0 {
		t.Fatalf("expected 0 inflight at boot; got %d", got)
	}
}

func TestInflightCount_TracksRegistrations(t *testing.T) {
	s := New(Options{})
	_, cancel := context.WithCancel(context.Background())
	defer cancel()
	if err := s.registerInflight("req-1", cancel); err != nil {
		t.Fatalf("registerInflight: %v", err)
	}
	if got := s.InflightCount(); got != 1 {
		t.Fatalf("expected 1 inflight after register; got %d", got)
	}
	s.unregisterInflight("req-1")
	if got := s.InflightCount(); got != 0 {
		t.Fatalf("expected 0 inflight after unregister; got %d", got)
	}
}
