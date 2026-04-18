# HealthApi

All URIs are relative to *http://localhost*

| Method | HTTP request | Description |
|------------- | ------------- | -------------|
| [**liveness**](HealthApi.md#liveness) | **GET** /health/live | Liveness probe - basic health check |
| [**readiness**](HealthApi.md#readiness) | **GET** /health/ready | Readiness probe - full health check |



## liveness

> HealthResponse liveness()

Liveness probe - basic health check

### Example

```ts
import {
  Configuration,
  HealthApi,
} from '@zlayer/client';
import type { LivenessRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const api = new HealthApi();

  try {
    const data = await api.liveness();
    console.log(data);
  } catch (error) {
    console.error(error);
  }
}

// Run the test
example().catch(console.error);
```

### Parameters

This endpoint does not need any parameter.

### Return type

[**HealthResponse**](HealthResponse.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Service is alive |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## readiness

> HealthResponse readiness()

Readiness probe - full health check

### Example

```ts
import {
  Configuration,
  HealthApi,
} from '@zlayer/client';
import type { ReadinessRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const api = new HealthApi();

  try {
    const data = await api.readiness();
    console.log(data);
  } catch (error) {
    console.error(error);
  }
}

// Run the test
example().catch(console.error);
```

### Parameters

This endpoint does not need any parameter.

### Return type

[**HealthResponse**](HealthResponse.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Service is ready |  -  |
| **503** | Service not ready |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)

