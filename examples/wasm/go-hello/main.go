//go:build tinygo

// Package main demonstrates a minimal Go WASM module for ZLayer.
//
// This example uses TinyGo to compile Go code to WebAssembly.
// TinyGo produces much smaller binaries than the standard Go compiler
// and supports WASI for system interactions.
//
// Building:
//
//	tinygo build -o hello.wasm -target=wasip2 main.go
package main

import (
	"fmt"
	"unsafe"
)

// Greeter handles greeting generation.
type Greeter struct {
	name string
}

// NewGreeter creates a new greeter with the given name.
func NewGreeter(name string) *Greeter {
	return &Greeter{name: name}
}

// Greet returns a greeting message.
func (g *Greeter) Greet() string {
	return fmt.Sprintf("Hello from Go WASM, %s!", g.name)
}

// Farewell returns a farewell message.
func (g *Greeter) Farewell() string {
	return fmt.Sprintf("Goodbye, %s!", g.name)
}

// GreetMany generates greetings for multiple names.
func GreetMany(names []string) []string {
	greetings := make([]string, len(names))
	for i, name := range names {
		g := NewGreeter(name)
		greetings[i] = g.Greet()
	}
	return greetings
}

// processInput handles a simple input/output flow.
// In a real application, this would read from WASI stdin
// and write to WASI stdout.
func processInput(input string) string {
	if input == "" {
		return "Hello from Go WASM!"
	}
	greeter := NewGreeter(input)
	return greeter.Greet()
}

// main is the entry point for the WASM module.
// When run as a command, it prints a greeting.
func main() {
	greeter := NewGreeter("ZLayer")
	fmt.Println(greeter.Greet())
}

// Export functions for WASM interface.
// These can be called directly from the host runtime.

//export greet
func greet(name *byte, nameLen int32) {
	// Convert the input to a Go string
	nameSlice := make([]byte, nameLen)
	for i := int32(0); i < nameLen; i++ {
		nameSlice[i] = *(*byte)(unsafe.Add(unsafe.Pointer(name), i))
	}
	goName := string(nameSlice)

	// Generate and print the greeting
	g := NewGreeter(goName)
	fmt.Println(g.Greet())
}

//export version
func version() {
	fmt.Println("go-hello-wasm v0.1.0")
}
