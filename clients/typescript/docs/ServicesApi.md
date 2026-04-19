# ServicesApi

All URIs are relative to *http://localhost*

| Method | HTTP request | Description |
|------------- | ------------- | -------------|
| [**getService**](ServicesApi.md#getservice) | **GET** /api/v1/deployments/{deployment}/services/{service} | Get service details. |
| [**getServiceLogs**](ServicesApi.md#getservicelogs) | **GET** /api/v1/deployments/{deployment}/services/{service}/logs | Get service logs. |
| [**listServices**](ServicesApi.md#listservices) | **GET** /api/v1/deployments/{deployment}/services | List services in a deployment. |
| [**scaleService**](ServicesApi.md#scaleservice) | **POST** /api/v1/deployments/{deployment}/services/{service}/scale | Scale a service. |



## getService

> ServiceDetails getService(deployment, service)

Get service details.

# Errors  Returns an error if the deployment or service is not found.

### Example

```ts
import {
  Configuration,
  ServicesApi,
} from '@zlayer/client';
import type { GetServiceRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new ServicesApi(config);

  const body = {
    // string | Deployment name
    deployment: deployment_example,
    // string | Service name
    service: service_example,
  } satisfies GetServiceRequest;

  try {
    const data = await api.getService(body);
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
| **deployment** | `string` | Deployment name | [Defaults to `undefined`] |
| **service** | `string` | Service name | [Defaults to `undefined`] |

### Return type

[**ServiceDetails**](ServiceDetails.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Service details |  -  |
| **401** | Unauthorized |  -  |
| **404** | Service not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## getServiceLogs

> string getServiceLogs(deployment, service, lines, follow, instance)

Get service logs.

Returns service logs as plain text when &#x60;follow&#x3D;false&#x60;, or as a Server-Sent Events stream when &#x60;follow&#x3D;true&#x60;.  In follow mode the server first emits the last N lines (controlled by the &#x60;lines&#x60; parameter) and then continuously polls for new output, emitting each new line as a &#x60;data:&#x60; SSE event.  # Errors  Returns an error if the deployment or service is not found, or the service manager is not available.

### Example

```ts
import {
  Configuration,
  ServicesApi,
} from '@zlayer/client';
import type { GetServiceLogsRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new ServicesApi(config);

  const body = {
    // string | Deployment name
    deployment: deployment_example,
    // string | Service name
    service: service_example,
    // number | Number of lines to return (optional)
    lines: 56,
    // boolean | Follow logs (streaming) (optional)
    follow: true,
    // string | Filter by container/instance (optional)
    instance: instance_example,
  } satisfies GetServiceLogsRequest;

  try {
    const data = await api.getServiceLogs(body);
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
| **deployment** | `string` | Deployment name | [Defaults to `undefined`] |
| **service** | `string` | Service name | [Defaults to `undefined`] |
| **lines** | `number` | Number of lines to return | [Optional] [Defaults to `undefined`] |
| **follow** | `boolean` | Follow logs (streaming) | [Optional] [Defaults to `undefined`] |
| **instance** | `string` | Filter by container/instance | [Optional] [Defaults to `undefined`] |

### Return type

**string**

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `text/plain`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Service logs (plain text or SSE stream) |  -  |
| **401** | Unauthorized |  -  |
| **404** | Service not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## listServices

> Array&lt;ServiceSummary&gt; listServices(deployment)

List services in a deployment.

# Errors  Returns an error if the deployment is not found or storage access fails.

### Example

```ts
import {
  Configuration,
  ServicesApi,
} from '@zlayer/client';
import type { ListServicesRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new ServicesApi(config);

  const body = {
    // string | Deployment name
    deployment: deployment_example,
  } satisfies ListServicesRequest;

  try {
    const data = await api.listServices(body);
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
| **deployment** | `string` | Deployment name | [Defaults to `undefined`] |

### Return type

[**Array&lt;ServiceSummary&gt;**](ServiceSummary.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | List of services |  -  |
| **401** | Unauthorized |  -  |
| **404** | Deployment not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## scaleService

> ServiceDetails scaleService(deployment, service, scaleRequest)

Scale a service.

# Errors  Returns an error if the deployment or service is not found, scaling fails, or the user lacks permission.

### Example

```ts
import {
  Configuration,
  ServicesApi,
} from '@zlayer/client';
import type { ScaleServiceRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new ServicesApi(config);

  const body = {
    // string | Deployment name
    deployment: deployment_example,
    // string | Service name
    service: service_example,
    // ScaleRequest
    scaleRequest: ...,
  } satisfies ScaleServiceRequest;

  try {
    const data = await api.scaleService(body);
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
| **deployment** | `string` | Deployment name | [Defaults to `undefined`] |
| **service** | `string` | Service name | [Defaults to `undefined`] |
| **scaleRequest** | [ScaleRequest](ScaleRequest.md) |  | |

### Return type

[**ServiceDetails**](ServiceDetails.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: `application/json`
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Service scaled |  -  |
| **400** | Invalid scale request |  -  |
| **401** | Unauthorized |  -  |
| **404** | Service not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)

