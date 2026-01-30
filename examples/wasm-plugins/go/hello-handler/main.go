//go:build tinygo

// Package main implements a simple HTTP handler WASM plugin for ZLayer using TinyGo.
//
// This example demonstrates implementing the wasi:http/incoming-handler interface
// to create a serverless HTTP handler that runs in ZLayer's WASM runtime.
//
// # Building
//
// TinyGo with WASI Preview 2 support is required:
//
//	# Install TinyGo (0.32+ recommended for wasip2 support)
//	# See: https://tinygo.org/getting-started/install/
//
//	# Generate WIT bindings (requires wit-bindgen-go)
//	go install go.bytecodealliance.org/cmd/wit-bindgen-go@latest
//	wit-bindgen-go generate --out gen ../../../../wit
//
//	# Build the WASM component
//	tinygo build -target=wasip2 -o hello-handler.wasm .
//
// # Endpoints
//
// - GET /         - Returns a welcome message with request info
// - GET /health   - Health check endpoint
// - POST /echo    - Echoes back the request body as JSON
// - Any other     - Returns 404 Not Found
//
// Note: TinyGo WASI HTTP support is still evolving. This example shows the
// intended structure. For production use, verify compatibility with your
// TinyGo and wasmtime versions.
package main

import (
	"encoding/json"

	"go.bytecodealliance.org/cm"
	"github.com/example/hello-handler/gen/wasi/http/types"
)

// init registers the HTTP handler with the WASI runtime.
func init() {
	types.SetExportsWasiHttp0_2_0_IncomingHandler(Handler{})
}

// Handler implements the wasi:http/incoming-handler interface.
type Handler struct{}

// Handle processes an incoming HTTP request and writes a response.
func (h Handler) Handle(request types.IncomingRequest, responseOut types.ResponseOutparam) {
	// Extract request information
	method := request.Method()
	pathWithQuery := request.PathWithQuery()
	path := "/"
	if pathWithQuery.IsSome() {
		path = *pathWithQuery.Some()
	}

	// Route the request
	var statusCode uint16
	var contentType string
	var body []byte

	methodStr := methodToString(method)

	switch {
	case methodStr == "GET" && (path == "/" || path == ""):
		statusCode = 200
		contentType = "application/json"
		response := map[string]string{
			"message": "Hello from Go WASM!",
			"method":  methodStr,
			"path":    path,
			"runtime": "wasi-preview2",
			"lang":    "tinygo",
		}
		body, _ = json.Marshal(response)

	case methodStr == "GET" && path == "/health":
		statusCode = 200
		contentType = "application/json"
		response := map[string]string{
			"status": "healthy",
			"plugin": "hello-handler-go",
		}
		body, _ = json.Marshal(response)

	case methodStr == "POST" && path == "/echo":
		statusCode = 200
		contentType = "application/json"
		requestBody := readRequestBody(request)
		response := map[string]interface{}{
			"echoed":  true,
			"length":  len(requestBody),
			"content": string(requestBody),
		}
		body, _ = json.Marshal(response)

	default:
		statusCode = 404
		contentType = "application/json"
		response := map[string]string{
			"error":  "Not Found",
			"method": methodStr,
			"path":   path,
		}
		body, _ = json.Marshal(response)
	}

	// Build response headers
	headers := types.NewFields()
	headers.Append(
		types.FieldKey("content-type"),
		types.FieldValue([]byte(contentType)),
	)
	headers.Append(
		types.FieldKey("x-zlayer-plugin"),
		types.FieldValue([]byte("hello-handler-go")),
	)

	// Create the response
	response := types.NewOutgoingResponse(headers)
	response.SetStatusCode(statusCode)

	// Write the response body
	outgoingBody := response.Body().OK()
	stream := outgoingBody.Write().OK()
	stream.BlockingWriteAndFlush(cm.ToList(body))
	stream.ResourceDrop()
	types.OutgoingBodyFinish(*outgoingBody, cm.None[types.Trailers]())

	// Send the response
	types.ResponseOutparamSet(responseOut, cm.OK[types.ErrorCode](response))
}

// methodToString converts a WASI HTTP method to a string.
func methodToString(method types.Method) string {
	switch {
	case method.Get() != nil:
		return "GET"
	case method.Post() != nil:
		return "POST"
	case method.Put() != nil:
		return "PUT"
	case method.Delete() != nil:
		return "DELETE"
	case method.Patch() != nil:
		return "PATCH"
	case method.Head() != nil:
		return "HEAD"
	case method.Options() != nil:
		return "OPTIONS"
	case method.Connect() != nil:
		return "CONNECT"
	case method.Trace() != nil:
		return "TRACE"
	default:
		if other := method.Other(); other != nil {
			return *other
		}
		return "UNKNOWN"
	}
}

// readRequestBody reads the full request body.
func readRequestBody(request types.IncomingRequest) []byte {
	bodyResult := request.Consume()
	if bodyResult.IsErr() {
		return nil
	}
	incomingBody := bodyResult.OK()

	streamResult := incomingBody.Stream()
	if streamResult.IsErr() {
		return nil
	}
	stream := streamResult.OK()

	var data []byte
	for {
		chunk := stream.BlockingRead(4096)
		if chunk.IsErr() {
			break
		}
		bytes := chunk.OK()
		if bytes.Len() == 0 {
			break
		}
		data = append(data, bytes.Slice()...)
	}

	stream.ResourceDrop()
	return data
}

// main is required but unused - the handler is registered via init().
func main() {}
