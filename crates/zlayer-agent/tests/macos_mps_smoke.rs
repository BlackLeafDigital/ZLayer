#![cfg(target_os = "macos")]
//! MPS GPU smoke tests for ZLayer macOS Seatbelt sandbox runtime
//!
//! These tests verify that actual Metal/MPS GPU workloads can execute
//! inside the Seatbelt sandbox. Unlike the profile-generation tests in
//! `macos_sandbox_e2e.rs`, these compile real Swift+Metal programs and
//! run them inside sandboxed containers to prove GPU access works end-to-end.
//!
//! # Requirements
//! - macOS with Apple Silicon (M-series)
//! - Xcode command line tools (`swiftc` available)
//!
//! # Running
//! ```bash
//! cargo test --package zlayer-agent --test macos_mps_smoke -- --nocapture
//! ```

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use zlayer_agent::{AgentError, ContainerId, MacSandboxConfig, Runtime, SandboxRuntime};
use zlayer_spec::DeploymentSpec;

/// Macro to run async test body with a timeout
macro_rules! with_timeout {
    ($timeout_secs:expr, $body:expr) => {{
        tokio::time::timeout(std::time::Duration::from_secs($timeout_secs), async move {
            $body
        })
        .await
        .expect(concat!(
            "Test timed out after ",
            stringify!($timeout_secs),
            " seconds"
        ))
    }};
}

const E2E_TEST_DIR: &str = "/tmp/zlayer-mps-smoke-test";

// =============================================================================
// Swift Metal source programs
// =============================================================================

/// Swift program that checks Metal device availability and basic MPS support.
/// Exits 0 and prints "METAL_OK" on success.
const SWIFT_METAL_DEVICE_CHECK: &str = r#"
import Metal

guard let device = MTLCreateSystemDefaultDevice() else {
    print("METAL_FAIL: No Metal device found")
    exit(1)
}

print("METAL_DEVICE: \(device.name)")
print("METAL_FAMILY: \(device.supportsFamily(.metal3) ? "Metal3+" : "pre-Metal3")")
print("METAL_UNIFIED: \(device.hasUnifiedMemory)")
print("METAL_OK")
"#;

/// Swift program that creates Metal buffers, fills them, and runs a trivial
/// compute operation (vector add) via a Metal compute pipeline.
/// This exercises actual GPU compute through the sandbox Seatbelt profile.
const SWIFT_MPS_COMPUTE: &str = r#"
import Metal
import MetalPerformanceShadersGraph

guard let device = MTLCreateSystemDefaultDevice() else {
    print("MPS_FAIL: No Metal device")
    exit(1)
}
print("MPS_DEVICE: \(device.name)")

guard let queue = device.makeCommandQueue() else {
    print("MPS_FAIL: Cannot create command queue")
    exit(1)
}

// Create two input buffers (float arrays)
let count = 1024
let byteLen = count * MemoryLayout<Float>.stride

guard let bufA = device.makeBuffer(length: byteLen, options: .storageModeShared),
      let bufB = device.makeBuffer(length: byteLen, options: .storageModeShared),
      let bufOut = device.makeBuffer(length: byteLen, options: .storageModeShared) else {
    print("MPS_FAIL: Cannot create buffers")
    exit(1)
}

// Fill input buffers
let ptrA = bufA.contents().bindMemory(to: Float.self, capacity: count)
let ptrB = bufB.contents().bindMemory(to: Float.self, capacity: count)
for i in 0..<count {
    ptrA[i] = Float(i)
    ptrB[i] = Float(count - i)
}

// Use MPSGraph for a simple vector addition (exercises MPS pre-compiled kernels)
let graph = MPSGraph()
let tensorA = graph.placeholder(shape: [count as NSNumber], dataType: .float32, name: "a")
let tensorB = graph.placeholder(shape: [count as NSNumber], dataType: .float32, name: "b")
let tensorSum = graph.addition(tensorA, tensorB, name: "sum")

let descA = MPSGraphTensorData(bufA, shape: [count as NSNumber], dataType: .float32)
let descB = MPSGraphTensorData(bufB, shape: [count as NSNumber], dataType: .float32)

guard let cmdBuf = queue.makeCommandBuffer() else {
    print("MPS_FAIL: Cannot create command buffer")
    exit(1)
}

let results = graph.encode(to: MPSCommandBuffer(commandBuffer: cmdBuf), feeds: [tensorA: descA, tensorB: descB], targetTensors: [tensorSum], targetOperations: nil, executionDescriptor: nil)

cmdBuf.commit()
cmdBuf.waitUntilCompleted()

if let error = cmdBuf.error {
    print("MPS_FAIL: Compute error: \(error)")
    exit(1)
}

// Verify: every element should equal count (i + (count - i) = count)
let outData = results[tensorSum]!
let outBuf = UnsafeMutableBufferPointer<Float>.allocate(capacity: count)
outData.mpsndarray().readBytes(outBuf.baseAddress!, strideBytes: nil)

var correct = 0
for i in 0..<count {
    if abs(outBuf[i] - Float(count)) < 0.001 { correct += 1 }
}
outBuf.deallocate()

print("MPS_RESULTS: \(correct)/\(count) correct")
if correct == count {
    print("MPS_COMPUTE_OK")
} else {
    print("MPS_FAIL: Only \(correct)/\(count) correct")
    exit(1)
}
"#;

/// Swift program that compiles a custom Metal compute shader at runtime.
/// This requires full MetalCompute access (MTLCompilerService).
const SWIFT_METAL_SHADER_COMPILE: &str = r#"
import Metal

guard let device = MTLCreateSystemDefaultDevice() else {
    print("SHADER_FAIL: No Metal device")
    exit(1)
}
print("SHADER_DEVICE: \(device.name)")

// Compile a trivial Metal shader at runtime (requires MTLCompilerService)
let shaderSrc = """
#include <metal_stdlib>
using namespace metal;

kernel void add_arrays(device const float* a [[buffer(0)]],
                       device const float* b [[buffer(1)]],
                       device float* out     [[buffer(2)]],
                       uint id [[thread_position_in_grid]]) {
    out[id] = a[id] + b[id];
}
"""

do {
    let library = try device.makeLibrary(source: shaderSrc, options: nil)
    guard let fn = library.makeFunction(name: "add_arrays") else {
        print("SHADER_FAIL: Function not found")
        exit(1)
    }
    let pipeline = try device.makeComputePipelineState(function: fn)
    print("SHADER_COMPILED: threadExecutionWidth=\(pipeline.threadExecutionWidth)")

    guard let queue = device.makeCommandQueue() else {
        print("SHADER_FAIL: No command queue")
        exit(1)
    }

    let count = 256
    let byteLen = count * MemoryLayout<Float>.stride
    guard let bufA = device.makeBuffer(length: byteLen, options: .storageModeShared),
          let bufB = device.makeBuffer(length: byteLen, options: .storageModeShared),
          let bufOut = device.makeBuffer(length: byteLen, options: .storageModeShared) else {
        print("SHADER_FAIL: Cannot create buffers")
        exit(1)
    }

    let ptrA = bufA.contents().bindMemory(to: Float.self, capacity: count)
    let ptrB = bufB.contents().bindMemory(to: Float.self, capacity: count)
    for i in 0..<count {
        ptrA[i] = Float(i) * 2.0
        ptrB[i] = Float(i) * 3.0
    }

    guard let cmdBuf = queue.makeCommandBuffer(),
          let encoder = cmdBuf.makeComputeCommandEncoder() else {
        print("SHADER_FAIL: Cannot create encoder")
        exit(1)
    }
    encoder.setComputePipelineState(pipeline)
    encoder.setBuffer(bufA, offset: 0, index: 0)
    encoder.setBuffer(bufB, offset: 0, index: 1)
    encoder.setBuffer(bufOut, offset: 0, index: 2)

    let gridSize = MTLSize(width: count, height: 1, depth: 1)
    let threadGroupSize = MTLSize(width: min(pipeline.maxTotalThreadsPerThreadgroup, count), height: 1, depth: 1)
    encoder.dispatchThreads(gridSize, threadsPerThreadgroup: threadGroupSize)
    encoder.endEncoding()

    cmdBuf.commit()
    cmdBuf.waitUntilCompleted()

    if let error = cmdBuf.error {
        print("SHADER_FAIL: \(error)")
        exit(1)
    }

    let ptrOut = bufOut.contents().bindMemory(to: Float.self, capacity: count)
    var correct = 0
    for i in 0..<count {
        let expected = Float(i) * 5.0  // 2i + 3i
        if abs(ptrOut[i] - expected) < 0.001 { correct += 1 }
    }

    print("SHADER_RESULTS: \(correct)/\(count) correct")
    if correct == count {
        print("SHADER_COMPUTE_OK")
    } else {
        print("SHADER_FAIL: Only \(correct)/\(count) correct")
        exit(1)
    }
} catch {
    print("SHADER_FAIL: \(error)")
    exit(1)
}
"#;

// =============================================================================
// Helpers
// =============================================================================

fn unique_name(prefix: &str) -> String {
    use rand::Rng;
    let suffix: u32 = rand::rng().random_range(10000..99999);
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
        % 1_000_000;
    format!("{}-{}-{}", prefix, timestamp, suffix)
}

fn create_e2e_runtime(gpu_access: bool) -> Result<SandboxRuntime, AgentError> {
    let test_dir = PathBuf::from(E2E_TEST_DIR);
    let config = MacSandboxConfig {
        data_dir: test_dir.join("data"),
        log_dir: test_dir.join("logs"),
        gpu_access,
    };
    SandboxRuntime::new(config)
}

/// Compile a Swift source file into a binary using swiftc.
/// Returns the path to the compiled binary.
fn compile_swift(source: &str, binary_name: &str) -> PathBuf {
    let tmp_dir = PathBuf::from(E2E_TEST_DIR).join("swift-build");
    std::fs::create_dir_all(&tmp_dir).expect("Failed to create swift build dir");

    let src_path = tmp_dir.join(format!("{}.swift", binary_name));
    let bin_path = tmp_dir.join(binary_name);

    std::fs::write(&src_path, source).expect("Failed to write Swift source");

    let output = std::process::Command::new("swiftc")
        .arg("-O")
        .arg("-o")
        .arg(&bin_path)
        .arg(&src_path)
        .arg("-framework")
        .arg("Metal")
        .arg("-framework")
        .arg("MetalPerformanceShadersGraph")
        .output()
        .expect("Failed to run swiftc -- are Xcode command line tools installed?");

    if !output.status.success() {
        panic!(
            "swiftc failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    bin_path
}

/// Prepare a rootfs containing the compiled test binary.
async fn prepare_gpu_rootfs(
    runtime: &SandboxRuntime,
    image_name: &str,
    binary_path: &std::path::Path,
) {
    let safe_name = image_name.replace(['/', ':', '@'], "_");
    let rootfs_dir = runtime
        .config()
        .data_dir
        .join("images")
        .join(&safe_name)
        .join("rootfs");

    tokio::fs::create_dir_all(rootfs_dir.join("bin"))
        .await
        .expect("Failed to create rootfs bin dir");

    // Copy the compiled Metal test binary
    let dst = rootfs_dir
        .join("bin")
        .join(binary_path.file_name().unwrap());
    tokio::fs::copy(binary_path, &dst)
        .await
        .expect("Failed to copy Metal binary to rootfs");

    // Also copy basic system binaries needed
    for bin in &["echo", "sh"] {
        let src = format!("/bin/{}", bin);
        let dst = rootfs_dir.join("bin").join(bin);
        if std::path::Path::new(&src).exists() && !dst.exists() {
            tokio::fs::copy(&src, &dst).await.ok();
        }
    }
}

struct ContainerGuard {
    runtime: Arc<SandboxRuntime>,
    id: ContainerId,
}

impl ContainerGuard {
    fn new(runtime: Arc<SandboxRuntime>, id: ContainerId) -> Self {
        Self { runtime, id }
    }
}

impl Drop for ContainerGuard {
    fn drop(&mut self) {
        let runtime = self.runtime.clone();
        let id = self.id.clone();
        tokio::spawn(async move {
            let _ = runtime.stop_container(&id, Duration::from_secs(5)).await;
            let _ = runtime.remove_container(&id).await;
        });
    }
}

// =============================================================================
// Tests
// =============================================================================

/// Verify Metal device is accessible from inside the Seatbelt sandbox.
///
/// Compiles a Swift program that calls MTLCreateSystemDefaultDevice() and
/// runs it inside a sandboxed container with full Metal compute access.
#[tokio::test]
async fn test_metal_device_available_in_sandbox() {
    with_timeout!(120, {
        // Compile the Swift Metal test binary
        let binary = compile_swift(SWIFT_METAL_DEVICE_CHECK, "metal-device-check");
        println!("Compiled Metal device check: {}", binary.display());

        let runtime = Arc::new(create_e2e_runtime(true).expect("Failed to create runtime"));

        let service_name = unique_name("metal-dev");
        let id = ContainerId {
            service: service_name,
            replica: 1,
        };

        let image_name = "macos-native/metal-device-check:latest";
        let yaml = format!(
            r#"
version: v1
deployment: mps-smoke
services:
  metal-dev:
    rtype: service
    image:
      name: {}
    command:
      entrypoint: ["bin/metal-device-check"]
    endpoints:
      - name: dummy
        protocol: tcp
        port: 8080
    resources:
      gpu:
        vendor: apple
        count: 1
    scale:
      mode: fixed
      replicas: 1
"#,
            image_name
        );

        let spec = serde_yaml::from_str::<DeploymentSpec>(&yaml)
            .expect("Failed to parse spec")
            .services
            .remove("metal-dev")
            .expect("Missing service");

        prepare_gpu_rootfs(&runtime, image_name, &binary).await;

        let _guard = ContainerGuard::new(runtime.clone(), id.clone());

        runtime
            .create_container(&id, &spec)
            .await
            .expect("create failed");
        runtime.start_container(&id).await.expect("start failed");

        let exit_code = runtime.wait_container(&id).await.expect("wait failed");
        println!("Metal device check exit code: {}", exit_code);

        let logs = runtime.container_logs(&id, 100).await.expect("logs failed");
        println!("--- Metal Device Check Output ---\n{}", logs);

        assert_eq!(exit_code, 0, "Metal device check should exit 0");
        assert!(
            logs.contains("METAL_OK"),
            "Output should contain METAL_OK, got:\n{}",
            logs
        );
        assert!(
            logs.contains("METAL_DEVICE:"),
            "Output should contain device name"
        );

        let _ = runtime.stop_container(&id, Duration::from_secs(5)).await;
        let _ = runtime.remove_container(&id).await;
    });
}

/// Run an actual MPS compute workload (vector addition via MPSGraph) inside
/// the Seatbelt sandbox with MPS-only GPU access.
///
/// This proves that pre-compiled MPS kernels execute correctly when the
/// sandbox restricts access to MPS-only mode (no shader compilation).
#[tokio::test]
async fn test_mps_compute_in_sandbox() {
    with_timeout!(120, {
        let binary = compile_swift(SWIFT_MPS_COMPUTE, "mps-compute");
        println!("Compiled MPS compute test: {}", binary.display());

        let runtime = Arc::new(create_e2e_runtime(true).expect("Failed to create runtime"));

        let service_name = unique_name("mps-compute");
        let id = ContainerId {
            service: service_name,
            replica: 1,
        };

        let image_name = "macos-native/mps-compute:latest";
        let yaml = format!(
            r#"
version: v1
deployment: mps-smoke
services:
  mps-compute:
    rtype: service
    image:
      name: {}
    command:
      entrypoint: ["bin/mps-compute"]
    endpoints:
      - name: dummy
        protocol: tcp
        port: 8080
    resources:
      gpu:
        vendor: apple
        count: 1
        mode: mps
    scale:
      mode: fixed
      replicas: 1
"#,
            image_name
        );

        let spec = serde_yaml::from_str::<DeploymentSpec>(&yaml)
            .expect("Failed to parse spec")
            .services
            .remove("mps-compute")
            .expect("Missing service");

        prepare_gpu_rootfs(&runtime, image_name, &binary).await;

        let _guard = ContainerGuard::new(runtime.clone(), id.clone());

        runtime
            .create_container(&id, &spec)
            .await
            .expect("create failed");
        runtime.start_container(&id).await.expect("start failed");

        let exit_code = runtime.wait_container(&id).await.expect("wait failed");
        println!("MPS compute exit code: {}", exit_code);

        let logs = runtime.container_logs(&id, 100).await.expect("logs failed");
        println!("--- MPS Compute Output ---\n{}", logs);

        assert_eq!(exit_code, 0, "MPS compute should exit 0");
        assert!(
            logs.contains("MPS_COMPUTE_OK"),
            "Output should contain MPS_COMPUTE_OK, got:\n{}",
            logs
        );
        assert!(
            logs.contains("1024/1024 correct"),
            "All 1024 vector additions should be correct"
        );

        let _ = runtime.stop_container(&id, Duration::from_secs(5)).await;
        let _ = runtime.remove_container(&id).await;
    });
}

/// Run a Metal compute shader that is compiled at runtime inside the sandbox.
///
/// This requires full MetalCompute access (not MPS-only) because it needs
/// the MTLCompilerService to compile the shader source at runtime.
/// This is the access level needed for MLX, custom Metal shaders, etc.
#[tokio::test]
async fn test_metal_shader_compile_in_sandbox() {
    with_timeout!(120, {
        let binary = compile_swift(SWIFT_METAL_SHADER_COMPILE, "metal-shader-compile");
        println!("Compiled Metal shader test: {}", binary.display());

        let runtime = Arc::new(create_e2e_runtime(true).expect("Failed to create runtime"));

        let service_name = unique_name("shader-compile");
        let id = ContainerId {
            service: service_name,
            replica: 1,
        };

        let image_name = "macos-native/metal-shader-compile:latest";
        // No gpu.mode = full Metal compute (default)
        let yaml = format!(
            r#"
version: v1
deployment: mps-smoke
services:
  shader-compile:
    rtype: service
    image:
      name: {}
    command:
      entrypoint: ["bin/metal-shader-compile"]
    endpoints:
      - name: dummy
        protocol: tcp
        port: 8080
    resources:
      gpu:
        vendor: apple
        count: 1
    scale:
      mode: fixed
      replicas: 1
"#,
            image_name
        );

        let spec = serde_yaml::from_str::<DeploymentSpec>(&yaml)
            .expect("Failed to parse spec")
            .services
            .remove("shader-compile")
            .expect("Missing service");

        prepare_gpu_rootfs(&runtime, image_name, &binary).await;

        let _guard = ContainerGuard::new(runtime.clone(), id.clone());

        runtime
            .create_container(&id, &spec)
            .await
            .expect("create failed");
        runtime.start_container(&id).await.expect("start failed");

        let exit_code = runtime.wait_container(&id).await.expect("wait failed");
        println!("Metal shader compile exit code: {}", exit_code);

        let logs = runtime.container_logs(&id, 100).await.expect("logs failed");
        println!("--- Metal Shader Compile Output ---\n{}", logs);

        assert_eq!(exit_code, 0, "Metal shader compile should exit 0");
        assert!(
            logs.contains("SHADER_COMPUTE_OK"),
            "Output should contain SHADER_COMPUTE_OK, got:\n{}",
            logs
        );
        assert!(
            logs.contains("SHADER_COMPILED:"),
            "Output should confirm shader was compiled"
        );
        assert!(
            logs.contains("256/256 correct"),
            "All 256 compute results should be correct"
        );

        let _ = runtime.stop_container(&id, Duration::from_secs(5)).await;
        let _ = runtime.remove_container(&id).await;
    });
}

/// Verify that GPU access is denied when the runtime has gpu_access=false.
///
/// Even if the deployment spec requests a GPU, a runtime configured with
/// `gpu_access: false` should NOT include any GPU rules in the Seatbelt
/// profile, causing MTLCreateSystemDefaultDevice() to return nil.
#[tokio::test]
async fn test_gpu_denied_when_runtime_disabled() {
    with_timeout!(120, {
        let binary = compile_swift(SWIFT_METAL_DEVICE_CHECK, "gpu-denied");
        println!("Compiled GPU denied test: {}", binary.display());

        // Create runtime with GPU DISABLED
        let runtime = Arc::new(create_e2e_runtime(false).expect("Failed to create runtime"));

        let service_name = unique_name("gpu-denied");
        let id = ContainerId {
            service: service_name,
            replica: 1,
        };

        let image_name = "macos-native/gpu-denied:latest";
        // Spec requests GPU, but runtime has gpu_access=false
        let yaml = format!(
            r#"
version: v1
deployment: mps-smoke
services:
  gpu-denied:
    rtype: service
    image:
      name: {}
    command:
      entrypoint: ["bin/gpu-denied"]
    endpoints:
      - name: dummy
        protocol: tcp
        port: 8080
    resources:
      gpu:
        vendor: apple
        count: 1
    scale:
      mode: fixed
      replicas: 1
"#,
            image_name
        );

        let spec = serde_yaml::from_str::<DeploymentSpec>(&yaml)
            .expect("Failed to parse spec")
            .services
            .remove("gpu-denied")
            .expect("Missing service");

        prepare_gpu_rootfs(&runtime, image_name, &binary).await;

        let _guard = ContainerGuard::new(runtime.clone(), id.clone());

        runtime
            .create_container(&id, &spec)
            .await
            .expect("create failed");
        runtime.start_container(&id).await.expect("start failed");

        let exit_code = runtime.wait_container(&id).await.expect("wait failed");
        println!("GPU denied exit code: {}", exit_code);

        let logs = runtime.container_logs(&id, 100).await.expect("logs failed");
        println!("--- GPU Denied Output ---\n{}", logs);

        // The process should fail because runtime disabled GPU access
        assert_ne!(
            exit_code, 0,
            "Metal should fail when runtime disables GPU, but got exit 0:\n{}",
            logs
        );
        assert!(
            logs.contains("METAL_FAIL"),
            "Output should contain METAL_FAIL, got:\n{}",
            logs
        );

        let _ = runtime.stop_container(&id, Duration::from_secs(5)).await;
        let _ = runtime.remove_container(&id).await;
    });
}
