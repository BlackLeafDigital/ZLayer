# ZLayer API Clients

Auto-generated API clients for ZLayer. These clients provide type-safe access to the ZLayer API from various programming languages.

## Generation

Clients are generated from the OpenAPI specification using [OpenAPI Generator](https://openapi-generator.tech/).

### Prerequisites

Install OpenAPI Generator:

```bash
# macOS
brew install openapi-generator

# npm (cross-platform)
npm install -g @openapitools/openapi-generator-cli

# Docker (no installation required)
docker pull openapitools/openapi-generator-cli
```

### Generate Clients

```bash
# Generate all clients
./scripts/generate-clients.sh

# Generate specific language
./scripts/generate-clients.sh typescript
./scripts/generate-clients.sh python
./scripts/generate-clients.sh go
./scripts/generate-clients.sh rust
./scripts/generate-clients.sh csharp
./scripts/generate-clients.sh java

# Use custom OpenAPI spec URL
./scripts/generate-clients.sh all http://localhost:3669/api-docs/openapi.json
```

## Client Usage

### TypeScript / JavaScript

```typescript
import { Configuration, FunctionsApi, ContainersApi } from '@zlayer/client';

const config = new Configuration({
  basePath: 'http://localhost:3669',
  headers: {
    'Authorization': 'Bearer YOUR_API_KEY'
  }
});

const functionsApi = new FunctionsApi(config);
const containersApi = new ContainersApi(config);

// Deploy a function
const deployment = await functionsApi.deployFunction({
  name: 'my-function',
  image: 'my-registry/my-function:latest',
  runtime: 'nodejs20',
  memory: 256,
  timeout: 30
});

// Invoke a function
const result = await functionsApi.invokeFunction('my-function', {
  body: { message: 'Hello, ZLayer!' }
});

// List containers
const containers = await containersApi.listContainers();
```

**Installation:**
```bash
cd clients/typescript
npm install
npm run build
```

### Python

```python
from zlayer_client import ApiClient, Configuration
from zlayer_client.api import FunctionsApi, ContainersApi

config = Configuration(
    host="http://localhost:3669",
    api_key={"Authorization": "Bearer YOUR_API_KEY"}
)

with ApiClient(config) as client:
    functions_api = FunctionsApi(client)
    containers_api = ContainersApi(client)

    # Deploy a function
    deployment = functions_api.deploy_function(
        name="my-function",
        image="my-registry/my-function:latest",
        runtime="python311",
        memory=256,
        timeout=30
    )

    # Invoke a function
    result = functions_api.invoke_function(
        "my-function",
        body={"message": "Hello, ZLayer!"}
    )

    # List containers
    containers = containers_api.list_containers()
```

**Installation:**
```bash
cd clients/python
pip install -e .
```

### Go

```go
package main

import (
    "context"
    "fmt"
    zlayer "github.com/BlackLeafDigital/zlayer-go"
)

func main() {
    config := zlayer.NewConfiguration()
    config.BasePath = "http://localhost:3669"
    config.AddDefaultHeader("Authorization", "Bearer YOUR_API_KEY")

    client := zlayer.NewAPIClient(config)
    ctx := context.Background()

    // Deploy a function
    deployment, _, err := client.FunctionsApi.DeployFunction(ctx).
        Name("my-function").
        Image("my-registry/my-function:latest").
        Runtime("go121").
        Memory(256).
        Timeout(30).
        Execute()
    if err != nil {
        panic(err)
    }

    // Invoke a function
    result, _, err := client.FunctionsApi.InvokeFunction(ctx, "my-function").
        Body(map[string]interface{}{"message": "Hello, ZLayer!"}).
        Execute()
    if err != nil {
        panic(err)
    }

    // List containers
    containers, _, err := client.ContainersApi.ListContainers(ctx).Execute()
    if err != nil {
        panic(err)
    }

    fmt.Printf("Deployed: %s\n", deployment.Id)
    fmt.Printf("Result: %v\n", result)
    fmt.Printf("Containers: %d\n", len(containers))
}
```

**Installation:**
```bash
go get github.com/BlackLeafDigital/zlayer-go
```

### Rust

```rust
use zlayer_client::{apis::configuration::Configuration, apis::functions_api, apis::containers_api};
use std::collections::HashMap;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut config = Configuration::default();
    config.base_path = "http://localhost:3669".to_string();
    config.bearer_access_token = Some("YOUR_API_KEY".to_string());

    // Deploy a function
    let deployment = functions_api::deploy_function(
        &config,
        "my-function",
        "my-registry/my-function:latest",
        Some("rust"),
        Some(256),
        Some(30),
    ).await?;

    // Invoke a function
    let mut body = HashMap::new();
    body.insert("message", "Hello, ZLayer!");
    let result = functions_api::invoke_function(&config, "my-function", body).await?;

    // List containers
    let containers = containers_api::list_containers(&config).await?;

    println!("Deployed: {}", deployment.id);
    println!("Result: {:?}", result);
    println!("Containers: {}", containers.len());

    Ok(())
}
```

**Installation (Cargo.toml):**
```toml
[dependencies]
zlayer-client = { path = "../clients/rust" }
tokio = { version = "1", features = ["full"] }
```

### C# / .NET

```csharp
using ZLayer.Client.Api;
using ZLayer.Client.Client;
using ZLayer.Client.Model;

var config = new Configuration
{
    BasePath = "http://localhost:3669",
    DefaultHeaders = new Dictionary<string, string>
    {
        { "Authorization", "Bearer YOUR_API_KEY" }
    }
};

var functionsApi = new FunctionsApi(config);
var containersApi = new ContainersApi(config);

// Deploy a function
var deployment = await functionsApi.DeployFunctionAsync(
    name: "my-function",
    image: "my-registry/my-function:latest",
    runtime: "dotnet8",
    memory: 256,
    timeout: 30
);

// Invoke a function
var result = await functionsApi.InvokeFunctionAsync(
    "my-function",
    new Dictionary<string, object> { { "message", "Hello, ZLayer!" } }
);

// List containers
var containers = await containersApi.ListContainersAsync();

Console.WriteLine($"Deployed: {deployment.Id}");
Console.WriteLine($"Result: {result}");
Console.WriteLine($"Containers: {containers.Count}");
```

**Installation:**
```bash
cd clients/csharp
dotnet build
dotnet pack
```

### Java

```java
import com.zlayer.client.ApiClient;
import com.zlayer.client.Configuration;
import com.zlayer.client.api.FunctionsApi;
import com.zlayer.client.api.ContainersApi;
import com.zlayer.client.model.*;

import java.util.Map;
import java.util.HashMap;

public class Example {
    public static void main(String[] args) throws Exception {
        ApiClient client = Configuration.getDefaultApiClient();
        client.setBasePath("http://localhost:3669");
        client.addDefaultHeader("Authorization", "Bearer YOUR_API_KEY");

        FunctionsApi functionsApi = new FunctionsApi(client);
        ContainersApi containersApi = new ContainersApi(client);

        // Deploy a function
        Deployment deployment = functionsApi.deployFunction(
            "my-function",
            "my-registry/my-function:latest",
            "java21",
            256,
            30
        );

        // Invoke a function
        Map<String, Object> body = new HashMap<>();
        body.put("message", "Hello, ZLayer!");
        Object result = functionsApi.invokeFunction("my-function", body);

        // List containers
        List<Container> containers = containersApi.listContainers();

        System.out.println("Deployed: " + deployment.getId());
        System.out.println("Result: " + result);
        System.out.println("Containers: " + containers.size());
    }
}
```

**Installation (Maven):**
```xml
<dependency>
    <groupId>com.zlayer</groupId>
    <artifactId>zlayer-client</artifactId>
    <version>0.1.0</version>
</dependency>
```

## Authentication

All clients support the following authentication methods:

### API Key

```bash
# Header-based
Authorization: Bearer YOUR_API_KEY

# Query parameter (not recommended)
?api_key=YOUR_API_KEY
```

### mTLS (Mutual TLS)

For production deployments, configure client certificates:

```typescript
// TypeScript example with mTLS
import https from 'https';
import fs from 'fs';

const httpsAgent = new https.Agent({
  cert: fs.readFileSync('client.crt'),
  key: fs.readFileSync('client.key'),
  ca: fs.readFileSync('ca.crt')
});

const config = new Configuration({
  basePath: 'https://api.zlayer.dev',
  fetchApi: (url, init) => fetch(url, { ...init, agent: httpsAgent })
});
```

## Error Handling

All clients throw typed exceptions for API errors:

```typescript
// TypeScript
try {
  await functionsApi.invokeFunction('nonexistent');
} catch (error) {
  if (error instanceof ResponseError) {
    console.error(`API Error ${error.response.status}: ${await error.response.text()}`);
  }
}
```

```python
# Python
from zlayer_client.exceptions import ApiException

try:
    functions_api.invoke_function('nonexistent')
except ApiException as e:
    print(f"API Error {e.status}: {e.body}")
```

## Regenerating Clients

After updating the OpenAPI specification:

```bash
# Remove old clients
rm -rf clients/*/

# Regenerate all
./scripts/generate-clients.sh all

# Or regenerate specific language
./scripts/generate-clients.sh typescript
```

## Contributing

Client generation templates can be customized in `scripts/templates/` (if present). See [OpenAPI Generator Customization](https://openapi-generator.tech/docs/customization) for details.

## License

Apache-2.0 - See [LICENSE](../LICENSE) for details.
