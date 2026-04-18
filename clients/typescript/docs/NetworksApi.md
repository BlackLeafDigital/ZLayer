# NetworksApi

All URIs are relative to *http://localhost*

| Method | HTTP request | Description |
|------------- | ------------- | -------------|
| [**createNetwork**](NetworksApi.md#createnetwork) | **POST** /api/v1/networks | Create a new network. |
| [**deleteNetwork**](NetworksApi.md#deletenetwork) | **DELETE** /api/v1/networks/{name} | Delete a network. |
| [**getNetwork**](NetworksApi.md#getnetwork) | **GET** /api/v1/networks/{name} | Get a specific network by name. |
| [**listNetworks**](NetworksApi.md#listnetworks) | **GET** /api/v1/networks | List all networks. |
| [**updateNetwork**](NetworksApi.md#updatenetwork) | **PUT** /api/v1/networks/{name} | Update an existing network. |



## createNetwork

> any createNetwork(body)

Create a new network.

The network name must be unique. Returns the created spec.  # Errors  Returns an error if validation fails, the network already exists, or the user lacks the operator role.

### Example

```ts
import {
  Configuration,
  NetworksApi,
} from '@zlayer/client';
import type { CreateNetworkRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new NetworksApi(config);

  const body = {
    // any
    body: ...,
  } satisfies CreateNetworkRequest;

  try {
    const data = await api.createNetwork(body);
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
| **body** | `any` |  | |

### Return type

**any**

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: `application/json`
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **201** | Network created |  -  |
| **400** | Invalid request |  -  |
| **401** | Unauthorized |  -  |
| **403** | Forbidden |  -  |
| **409** | Network already exists |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## deleteNetwork

> deleteNetwork(name)

Delete a network.

Permanently removes a network definition.  # Errors  Returns an error if the network is not found or the user lacks the operator role.

### Example

```ts
import {
  Configuration,
  NetworksApi,
} from '@zlayer/client';
import type { DeleteNetworkRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new NetworksApi(config);

  const body = {
    // string | Network name
    name: name_example,
  } satisfies DeleteNetworkRequest;

  try {
    const data = await api.deleteNetwork(body);
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
| **name** | `string` | Network name | [Defaults to `undefined`] |

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
| **204** | Network deleted |  -  |
| **401** | Unauthorized |  -  |
| **403** | Forbidden |  -  |
| **404** | Network not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## getNetwork

> any getNetwork(name)

Get a specific network by name.

Returns the full &#x60;NetworkPolicySpec&#x60; for the named network.  # Errors  Returns an error if the network is not found or the user is not authenticated.

### Example

```ts
import {
  Configuration,
  NetworksApi,
} from '@zlayer/client';
import type { GetNetworkRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new NetworksApi(config);

  const body = {
    // string | Network name
    name: name_example,
  } satisfies GetNetworkRequest;

  try {
    const data = await api.getNetwork(body);
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
| **name** | `string` | Network name | [Defaults to `undefined`] |

### Return type

**any**

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Network details |  -  |
| **401** | Unauthorized |  -  |
| **404** | Network not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## listNetworks

> Array&lt;NetworkSummary&gt; listNetworks()

List all networks.

Returns a summary of every network defined in the system.  # Errors  Returns an error if the user is not authenticated.

### Example

```ts
import {
  Configuration,
  NetworksApi,
} from '@zlayer/client';
import type { ListNetworksRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new NetworksApi(config);

  try {
    const data = await api.listNetworks();
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

[**Array&lt;NetworkSummary&gt;**](NetworkSummary.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | List of networks |  -  |
| **401** | Unauthorized |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## updateNetwork

> any updateNetwork(name, body)

Update an existing network.

Replaces the network definition for the given name. The name in the URL must match the name in the body.  # Errors  Returns an error if the network is not found, the URL name does not match the body, or the user lacks the operator role.

### Example

```ts
import {
  Configuration,
  NetworksApi,
} from '@zlayer/client';
import type { UpdateNetworkRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new NetworksApi(config);

  const body = {
    // string | Network name
    name: name_example,
    // any
    body: ...,
  } satisfies UpdateNetworkRequest;

  try {
    const data = await api.updateNetwork(body);
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
| **name** | `string` | Network name | [Defaults to `undefined`] |
| **body** | `any` |  | |

### Return type

**any**

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: `application/json`
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Network updated |  -  |
| **400** | Invalid request |  -  |
| **401** | Unauthorized |  -  |
| **403** | Forbidden |  -  |
| **404** | Network not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)

