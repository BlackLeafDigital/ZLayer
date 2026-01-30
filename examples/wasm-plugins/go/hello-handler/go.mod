module github.com/BlackLeafDigital/ZLayer/examples/wasm-plugins/go/hello-handler

go 1.22

// Note: The exact versions may need adjustment based on your TinyGo version
// and the state of the Go WASM ecosystem.
//
// To generate bindings:
//   go install go.bytecodealliance.org/cmd/wit-bindgen-go@latest
//   wit-bindgen-go generate --out gen ../../../../wit
//
// The generated bindings will be placed in the ./gen directory.
// Import paths in main.go should be adjusted to match your module path.

require go.bytecodealliance.org/cm v0.1.0

// Replace directive for local development if needed:
// replace github.com/example/hello-handler/gen => ./gen
