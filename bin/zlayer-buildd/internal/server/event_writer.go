package server

import (
	"bytes"
	"sync"

	pb "github.com/zorpxinc/zlayer/zlayer-buildd/internal/pb"
)

// eventWriter is an io.Writer that splits incoming bytes on '\n' and emits
// one pb.LogLine event per complete line. Partial lines are buffered until
// the next write or Flush().
//
// Concurrency: buildah's Out/Err/ReportWriter may all be the *same* writer
// or be written from different goroutines, so we guard the buffer with a
// mutex.
type eventWriter struct {
	mu       sync.Mutex
	stream   pb.BuildService_BuildServer
	isStderr bool
	buf      bytes.Buffer
}

func newEventWriter(stream pb.BuildService_BuildServer, isStderr bool) *eventWriter {
	return &eventWriter{stream: stream, isStderr: isStderr}
}

// Write implements io.Writer. It accumulates bytes until a newline appears,
// emits one LogLine event per complete line, and reports back the full
// length so callers see a clean io.Writer contract (never short writes).
// Stream send errors are intentionally swallowed: the gRPC layer surfaces
// them via the stream Context being cancelled, which Build observes and
// translates into a return error.
func (w *eventWriter) Write(p []byte) (int, error) {
	w.mu.Lock()
	defer w.mu.Unlock()
	n := len(p)
	w.buf.Write(p)
	for {
		raw, err := w.buf.ReadBytes('\n')
		if err != nil {
			// Partial line — put it back and wait for more.
			if len(raw) > 0 {
				w.buf.Write(raw)
			}
			break
		}
		trimmed := bytes.TrimRight(raw, "\r\n")
		if len(trimmed) == 0 {
			continue
		}
		_ = w.stream.Send(&pb.BuildEvent{
			Event: &pb.BuildEvent_Log{
				Log: &pb.LogLine{Line: string(trimmed), IsStderr: w.isStderr},
			},
		})
	}
	return n, nil
}

// Flush emits any buffered partial line as a single Log event. Called by
// Build at the very end so the last line (often the image ID) is not lost.
func (w *eventWriter) Flush() {
	w.mu.Lock()
	defer w.mu.Unlock()
	if w.buf.Len() == 0 {
		return
	}
	line := bytes.TrimRight(w.buf.Bytes(), "\r\n")
	w.buf.Reset()
	if len(line) == 0 {
		return
	}
	_ = w.stream.Send(&pb.BuildEvent{
		Event: &pb.BuildEvent_Log{
			Log: &pb.LogLine{Line: string(line), IsStderr: w.isStderr},
		},
	})
}
