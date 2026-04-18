# InternalApi

All URIs are relative to *http://localhost*

| Method | HTTP request | Description |
|------------- | ------------- | -------------|
| [**getReplicasInternal**](InternalApi.md#getreplicasinternal) | **GET** /api/v1/internal/replicas/{service} | Get the current replica count for a service. |
| [**scaleServiceInternal**](InternalApi.md#scaleserviceinternal) | **POST** /api/v1/internal/scale | Scale a service via internal scheduler request. |



## getReplicasInternal

> InternalScaleResponse getReplicasInternal(service)

Get the current replica count for a service.

This endpoint allows the scheduler to query the current state of a service.  # Errors  Returns an error if the service is not found or authentication is invalid.

### Example

```ts
import {
  Configuration,
  InternalApi,
} from '@zlayer/client';
import type { GetReplicasInternalRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const api = new InternalApi();

  const body = {
    // string | Service name
    service: service_example,
  } satisfies GetReplicasInternalRequest;

  try {
    const data = await api.getReplicasInternal(body);
    console.log(data);
  } catch (error) {
    console.error(error);
  }
}

// Run the test
example().catch(console.error);
```

### Parameters


| Name | Type | Description  | Notes |
|------------- | ------------- | ------------- | -------------|
| **service** | `string` | Service name | [Defaults to `undefined`] |

### Return type

[**InternalScaleResponse**](InternalScaleResponse.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Current replica count |  -  |
| **401** | Unauthorized - invalid or missing internal token |  -  |
| **404** | Service not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## scaleServiceInternal

> InternalScaleResponse scaleServiceInternal(internalScaleRequest)

Scale a service via internal scheduler request.

This endpoint is called by the distributed scheduler leader to trigger scaling operations on agent nodes. It uses a shared secret for authentication.  # Errors  Returns an error if the service is not found, scaling fails, or authentication is invalid.

### Example

```ts
import {
  Configuration,
  InternalApi,
} from '@zlayer/client';
import type { ScaleServiceInternalRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const api = new InternalApi();

  const body = {
    // InternalScaleRequest
    internalScaleRequest: ...,
  } satisfies ScaleServiceInternalRequest;

  try {
    const data = await api.scaleServiceInternal(body);
    console.log(data);
  } catch (error) {
    console.error(error);
  }
}

// Run the test
example().catch(console.error);
```

### Parameters


| Name | Type | Description  | Notes |
|------------- | ------------- | ------------- | -------------|
| **internalScaleRequest** | [InternalScaleRequest](InternalScaleRequest.md) |  | |

### Return type

[**InternalScaleResponse**](InternalScaleResponse.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: `application/json`
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Service scaled successfully |  -  |
| **401** | Unauthorized - invalid or missing internal token |  -  |
| **404** | Service not found |  -  |
| **500** | Internal error |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)

