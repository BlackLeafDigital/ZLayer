/**
 * Server-Sent Events (SSE) helpers for streaming endpoints.
 *
 * The ZLayer daemon emits SSE streams on a handful of endpoints:
 * `/containers/{id}/exec?stream=true`, `/containers/{id}/stats?stream=true`,
 * and `/events?follow=true`. The auto-generated OpenAPI client exposes a
 * `*Raw()` variant on each operation that returns the underlying
 * `Response` — we read the body as a ReadableStream of UTF-8 bytes, split
 * on the standard SSE frame delimiter (`\n\n`), and yield parsed frames.
 *
 * Frame format:
 *
 *   event: <name>\n
 *   data: <payload>\n
 *   \n
 *
 * Only `event` and `data` lines are parsed; other fields (`id`, `retry`,
 * comments) are ignored. Multi-line `data:` payloads (which concatenate
 * with newlines per the spec) are supported.
 */
export interface SseFrame {
  /** Event name (e.g. `"stdout"`, `"stderr"`, `"exit"`). Defaults to `"message"` when omitted. */
  event: string;
  /** Raw textual payload, with internal `\n` preserved when split across multiple `data:` lines. */
  data: string;
}

/**
 * Consume an SSE response body and yield one {@link SseFrame} per
 * complete frame. Terminates when the stream closes.
 *
 * Throws if the response has no body (e.g. HEAD request, or a server
 * that incorrectly returned an empty stream).
 */
export async function* consumeSseStream(
  response: Response,
): AsyncIterable<SseFrame> {
  if (!response.body) {
    throw new Error("SSE response has no body");
  }

  // Node 18+ ReadableStream supports async iteration directly; Node 16
  // requires `getReader()`. We go through the reader for portability.
  const reader = (response.body as unknown as ReadableStream<Uint8Array>).getReader();
  const decoder = new TextDecoder("utf-8");
  let buffer = "";

  try {
    while (true) {
      const { value, done } = await reader.read();
      if (done) {
        // Flush any trailing frame that didn't end with \n\n.
        const trailing = buffer.trim();
        if (trailing.length > 0) {
          const parsed = parseFrame(trailing);
          if (parsed !== null) yield parsed;
        }
        return;
      }
      buffer += decoder.decode(value, { stream: true });

      // SSE frames are delimited by a blank line. Per the spec, the
      // separator can be `\n\n` or `\r\n\r\n`; normalize CRLF first.
      buffer = buffer.replace(/\r\n/g, "\n");
      let idx: number;
      while ((idx = buffer.indexOf("\n\n")) !== -1) {
        const raw = buffer.slice(0, idx);
        buffer = buffer.slice(idx + 2);
        const parsed = parseFrame(raw);
        if (parsed !== null) yield parsed;
      }
    }
  } finally {
    // Best-effort cancel so the transport can release its socket promptly
    // if the consumer stops iterating early (e.g. via `break`).
    try {
      await reader.cancel();
    } catch {
      // ignore
    }
  }
}

/**
 * Parse a single SSE frame block (without the trailing blank line).
 * Returns `null` when the block has no `data:` payload (e.g. a comment-
 * only frame or a keep-alive ping).
 */
function parseFrame(block: string): SseFrame | null {
  let event = "message";
  const dataLines: string[] = [];
  for (const line of block.split("\n")) {
    if (line.length === 0) continue;
    // Comments: lines starting with `:` per the SSE spec.
    if (line.startsWith(":")) continue;

    const colon = line.indexOf(":");
    let field: string;
    let value: string;
    if (colon === -1) {
      field = line;
      value = "";
    } else {
      field = line.slice(0, colon);
      // Strip a single leading space after the colon, per the SSE spec.
      value = line.slice(colon + 1);
      if (value.startsWith(" ")) value = value.slice(1);
    }

    if (field === "event") {
      event = value;
    } else if (field === "data") {
      dataLines.push(value);
    }
    // `id` and `retry` and unknown fields are ignored.
  }

  if (dataLines.length === 0) return null;
  return { event, data: dataLines.join("\n") };
}
