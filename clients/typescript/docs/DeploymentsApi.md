# DeploymentsApi

All URIs are relative to *http://localhost*

| Method | HTTP request | Description |
|------------- | ------------- | -------------|
| [**createDeployment**](DeploymentsApi.md#createdeploymentoperation) | **POST** /api/v1/deployments | Create a new deployment. |
| [**deleteDeployment**](DeploymentsApi.md#deletedeployment) | **DELETE** /api/v1/deployments/{name} | Delete a deployment. |
| [**getDeployment**](DeploymentsApi.md#getdeployment) | **GET** /api/v1/deployments/{name} | Get deployment details (with live per-service health when available). |
| [**listDeployments**](DeploymentsApi.md#listdeployments) | **GET** /api/v1/deployments | List all deployments. |



## createDeployment

> DeploymentDetails createDeployment(createDeploymentRequest)

Create a new deployment.

When the daemon has orchestration wired (service manager, proxy, overlay), this handler:  1. Parses and validates the spec YAML  2. Stores the deployment with status &#x60;Deploying&#x60;  3. Spawns an async task that registers services, sets up overlays,     configures the proxy, and scales services  4. Returns immediately with &#x60;Deploying&#x60; status  5. The async task updates the stored status to &#x60;Running&#x60; or &#x60;Failed&#x60;  Without orchestration wired, it stores the spec with &#x60;Pending&#x60; status.  # Errors  Returns an error if the spec is invalid or storage fails.

### Example

```ts
import {
  Configuration,
  DeploymentsApi,
} from '@zlayer/client';
import type { CreateDeploymentOperationRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new DeploymentsApi(config);

  const body = {
    // CreateDeploymentRequest
    createDeploymentRequest: ...,
  } satisfies CreateDeploymentOperationRequest;

  try {
    const data = await api.createDeployment(body);
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
| **createDeploymentRequest** | [CreateDeploymentRequest](CreateDeploymentRequest.md) |  | |

### Return type

[**DeploymentDetails**](DeploymentDetails.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: `application/json`
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **201** | Deployment created |  -  |
| **400** | Invalid specification |  -  |
| **401** | Unauthorized |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## deleteDeployment

> deleteDeployment(name)

Delete a deployment.

# Errors  Returns an error if the deployment is not found, storage access fails, or teardown encounters critical failures.

### Example

```ts
import {
  Configuration,
  DeploymentsApi,
} from '@zlayer/client';
import type { DeleteDeploymentRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new DeploymentsApi(config);

  const body = {
    // string | Deployment name
    name: name_example,
  } satisfies DeleteDeploymentRequest;

  try {
    const data = await api.deleteDeployment(body);
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
| **name** | `string` | Deployment name | [Defaults to `undefined`] |

### Return type

`void` (Empty response body)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: Not defined


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **204** | Deployment deleted |  -  |
| **401** | Unauthorized |  -  |
| **404** | Deployment not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## getDeployment

> DeploymentDetails getDeployment(name)

Get deployment details (with live per-service health when available).

# Errors  Returns an error if the deployment is not found or storage access fails.

### Example

```ts
import {
  Configuration,
  DeploymentsApi,
} from '@zlayer/client';
import type { GetDeploymentRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new DeploymentsApi(config);

  const body = {
    // string | Deployment name
    name: name_example,
  } satisfies GetDeploymentRequest;

  try {
    const data = await api.getDeployment(body);
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
| **name** | `string` | Deployment name | [Defaults to `undefined`] |

### Return type

[**DeploymentDetails**](DeploymentDetails.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Deployment details |  -  |
| **401** | Unauthorized |  -  |
| **404** | Deployment not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## listDeployments

> Array&lt;DeploymentSummary&gt; listDeployments()

List all deployments.

# Errors  Returns an error if storage access fails.

### Example

```ts
import {
  Configuration,
  DeploymentsApi,
} from '@zlayer/client';
import type { ListDeploymentsRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new DeploymentsApi(config);

  try {
    const data = await api.listDeployments();
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

[**Array&lt;DeploymentSummary&gt;**](DeploymentSummary.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | List of deployments |  -  |
| **401** | Unauthorized |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)

