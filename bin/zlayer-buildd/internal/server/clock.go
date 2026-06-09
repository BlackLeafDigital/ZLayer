// Package server hosts the BuildService gRPC implementation that wraps
// `imagebuildah.BuildDockerfiles` from go.podman.io/buildah.
package server

import "time"

// nowUnixNanos returns the current wall-clock time as nanoseconds since the
// Unix epoch. Centralised here so tests can swap the clock if we ever need
// to.
func nowUnixNanos() int64 {
	return time.Now().UnixNano()
}
