#!/bin/bash
set -euo pipefail

# ZLayer OpenAPI Client Generator
# Generates typed API clients for multiple languages

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"
CLIENTS_DIR="$ROOT_DIR/clients"
OPENAPI_SPEC="$ROOT_DIR/openapi.json"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

info() { echo -e "${GREEN}[INFO]${NC} $*"; }
warn() { echo -e "${YELLOW}[WARN]${NC} $*"; }
error() { echo -e "${RED}[ERROR]${NC} $*" >&2; }

# Check if openapi-generator is installed
check_deps() {
    if ! command -v openapi-generator &> /dev/null; then
        error "openapi-generator not found. Install with:"
        echo "  brew install openapi-generator  # macOS"
        echo "  npm install -g @openapitools/openapi-generator-cli  # npm"
        exit 1
    fi
}

# Fetch OpenAPI spec from running server or use local file
fetch_spec() {
    local url="${1:-http://localhost:8080/api-docs/openapi.json}"

    if [[ -f "$OPENAPI_SPEC" ]]; then
        info "Using existing OpenAPI spec: $OPENAPI_SPEC"
    else
        info "Fetching OpenAPI spec from $url..."
        curl -fsSL "$url" -o "$OPENAPI_SPEC" || {
            error "Failed to fetch OpenAPI spec. Start zlayer serve first or provide spec file."
            exit 1
        }
    fi
}

# Generate TypeScript client
generate_typescript() {
    info "Generating TypeScript client..."
    mkdir -p "$CLIENTS_DIR/typescript"
    openapi-generator generate \
        -i "$OPENAPI_SPEC" \
        -g typescript-fetch \
        -o "$CLIENTS_DIR/typescript" \
        --additional-properties=npmName=@zlayer/client,npmVersion=0.1.0,supportsES6=true,typescriptThreePlus=true

    # Create package.json if not exists
    cat > "$CLIENTS_DIR/typescript/package.json" << 'EOF'
{
  "name": "@zlayer/client",
  "version": "0.1.0",
  "description": "ZLayer API client for TypeScript/JavaScript",
  "main": "dist/index.js",
  "types": "dist/index.d.ts",
  "scripts": {
    "build": "tsc",
    "prepublishOnly": "npm run build"
  },
  "devDependencies": {
    "typescript": "^5.0.0"
  },
  "repository": {
    "type": "git",
    "url": "https://github.com/BlackLeafDigital/ZLayer.git"
  },
  "license": "Apache-2.0"
}
EOF
    info "TypeScript client generated at $CLIENTS_DIR/typescript"
}

# Generate Python client
generate_python() {
    info "Generating Python HTTP client..."
    mkdir -p "$CLIENTS_DIR/python"
    openapi-generator generate \
        -i "$OPENAPI_SPEC" \
        -g python \
        -o "$CLIENTS_DIR/python" \
        --additional-properties=packageName=zlayer_client,packageVersion=0.1.0,projectName=zlayer-client
    info "Python client generated at $CLIENTS_DIR/python"
}

# Generate Go client
generate_go() {
    info "Generating Go client..."
    mkdir -p "$CLIENTS_DIR/go"
    openapi-generator generate \
        -i "$OPENAPI_SPEC" \
        -g go \
        -o "$CLIENTS_DIR/go" \
        --additional-properties=packageName=zlayer,packageVersion=0.1.0,isGoSubmodule=true

    # Add go.mod
    cat > "$CLIENTS_DIR/go/go.mod" << 'EOF'
module github.com/BlackLeafDigital/zlayer-go

go 1.21
EOF
    info "Go client generated at $CLIENTS_DIR/go"
}

# Generate Rust client
generate_rust() {
    info "Generating Rust client..."
    mkdir -p "$CLIENTS_DIR/rust"
    openapi-generator generate \
        -i "$OPENAPI_SPEC" \
        -g rust \
        -o "$CLIENTS_DIR/rust" \
        --additional-properties=packageName=zlayer-client,packageVersion=0.1.0
    info "Rust client generated at $CLIENTS_DIR/rust"
}

# Generate C#/.NET client
generate_csharp() {
    info "Generating C#/.NET client..."
    mkdir -p "$CLIENTS_DIR/csharp"
    openapi-generator generate \
        -i "$OPENAPI_SPEC" \
        -g csharp \
        -o "$CLIENTS_DIR/csharp" \
        --additional-properties=packageName=ZLayer.Client,packageVersion=0.1.0,targetFramework=net8.0
    info "C# client generated at $CLIENTS_DIR/csharp"
}

# Generate Java client
generate_java() {
    info "Generating Java client..."
    mkdir -p "$CLIENTS_DIR/java"
    openapi-generator generate \
        -i "$OPENAPI_SPEC" \
        -g java \
        -o "$CLIENTS_DIR/java" \
        --additional-properties=groupId=com.zlayer,artifactId=zlayer-client,artifactVersion=0.1.0
    info "Java client generated at $CLIENTS_DIR/java"
}

# Main
main() {
    local langs="${1:-all}"

    check_deps
    fetch_spec "${2:-}"

    mkdir -p "$CLIENTS_DIR"

    case "$langs" in
        all)
            generate_typescript
            generate_python
            generate_go
            generate_rust
            generate_csharp
            generate_java
            ;;
        typescript|ts) generate_typescript ;;
        python|py) generate_python ;;
        go) generate_go ;;
        rust|rs) generate_rust ;;
        csharp|cs|dotnet) generate_csharp ;;
        java) generate_java ;;
        *)
            error "Unknown language: $langs"
            echo "Usage: $0 [all|typescript|python|go|rust|csharp|java] [openapi-url]"
            exit 1
            ;;
    esac

    info "Client generation complete!"
}

main "$@"
