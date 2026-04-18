# TunnelsApi

All URIs are relative to *http://localhost*

| Method | HTTP request | Description |
|------------- | ------------- | -------------|
| [**createNodeTunnel**](TunnelsApi.md#createnodetunneloperation) | **POST** /api/v1/tunnels/node | Create a node-to-node tunnel. |
| [**createTunnel**](TunnelsApi.md#createtunneloperation) | **POST** /api/v1/tunnels | Create a new tunnel token. |
| [**getTunnelStatus**](TunnelsApi.md#gettunnelstatus) | **GET** /api/v1/tunnels/{id}/status | Get tunnel status. |
| [**listTunnels**](TunnelsApi.md#listtunnels) | **GET** /api/v1/tunnels | List all tunnels. |
| [**removeNodeTunnel**](TunnelsApi.md#removenodetunnel) | **DELETE** /api/v1/tunnels/node/{name} | Remove a node-to-node tunnel. |
| [**revokeTunnel**](TunnelsApi.md#revoketunnel) | **DELETE** /api/v1/tunnels/{id} | Revoke (delete) a tunnel. |



## createNodeTunnel

> CreateNodeTunnelResponse createNodeTunnel(createNodeTunnelRequest)

Create a node-to-node tunnel.

# Errors  Returns an error if validation fails or the user lacks permission.

### Example

```ts
import {
  Configuration,
  TunnelsApi,
} from '@zlayer/client';
import type { CreateNodeTunnelOperationRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new TunnelsApi(config);

  const body = {
    // CreateNodeTunnelRequest
    createNodeTunnelRequest: ...,
  } satisfies CreateNodeTunnelOperationRequest;

  try {
    const data = await api.createNodeTunnel(body);
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
| **createNodeTunnelRequest** | [CreateNodeTunnelRequest](CreateNodeTunnelRequest.md) |  | |

### Return type

[**CreateNodeTunnelResponse**](CreateNodeTunnelResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: `application/json`
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **201** | Node tunnel created |  -  |
| **400** | Invalid request |  -  |
| **401** | Unauthorized |  -  |
| **403** | Forbidden - admin role required |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## createTunnel

> CreateTunnelResponse createTunnel(createTunnelRequest)

Create a new tunnel token.

# Errors  Returns an error if validation fails or authentication is invalid.

### Example

```ts
import {
  Configuration,
  TunnelsApi,
} from '@zlayer/client';
import type { CreateTunnelOperationRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new TunnelsApi(config);

  const body = {
    // CreateTunnelRequest
    createTunnelRequest: ...,
  } satisfies CreateTunnelOperationRequest;

  try {
    const data = await api.createTunnel(body);
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
| **createTunnelRequest** | [CreateTunnelRequest](CreateTunnelRequest.md) |  | |

### Return type

[**CreateTunnelResponse**](CreateTunnelResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: `application/json`
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **201** | Tunnel token created |  -  |
| **400** | Invalid request |  -  |
| **401** | Unauthorized |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## getTunnelStatus

> TunnelStatus getTunnelStatus(id)

Get tunnel status.

# Errors  Returns an error if the tunnel is not found.

### Example

```ts
import {
  Configuration,
  TunnelsApi,
} from '@zlayer/client';
import type { GetTunnelStatusRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new TunnelsApi(config);

  const body = {
    // string | Tunnel identifier
    id: id_example,
  } satisfies GetTunnelStatusRequest;

  try {
    const data = await api.getTunnelStatus(body);
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
| **id** | `string` | Tunnel identifier | [Defaults to `undefined`] |

### Return type

[**TunnelStatus**](TunnelStatus.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Tunnel status |  -  |
| **401** | Unauthorized |  -  |
| **404** | Tunnel not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## listTunnels

> Array&lt;TunnelSummary&gt; listTunnels()

List all tunnels.

# Errors  Returns an error if authentication fails.

### Example

```ts
import {
  Configuration,
  TunnelsApi,
} from '@zlayer/client';
import type { ListTunnelsRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new TunnelsApi(config);

  try {
    const data = await api.listTunnels();
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

[**Array&lt;TunnelSummary&gt;**](TunnelSummary.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | List of tunnels |  -  |
| **401** | Unauthorized |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## removeNodeTunnel

> SuccessResponse removeNodeTunnel(name)

Remove a node-to-node tunnel.

# Errors  Returns an error if the tunnel is not found or the user lacks permission.

### Example

```ts
import {
  Configuration,
  TunnelsApi,
} from '@zlayer/client';
import type { RemoveNodeTunnelRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new TunnelsApi(config);

  const body = {
    // string | Tunnel name
    name: name_example,
  } satisfies RemoveNodeTunnelRequest;

  try {
    const data = await api.removeNodeTunnel(body);
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
| **name** | `string` | Tunnel name | [Defaults to `undefined`] |

### Return type

[**SuccessResponse**](SuccessResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Node tunnel removed |  -  |
| **401** | Unauthorized |  -  |
| **403** | Forbidden - admin role required |  -  |
| **404** | Tunnel not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## revokeTunnel

> SuccessResponse revokeTunnel(id)

Revoke (delete) a tunnel.

# Errors  Returns an error if the tunnel is not found.

### Example

```ts
import {
  Configuration,
  TunnelsApi,
} from '@zlayer/client';
import type { RevokeTunnelRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new TunnelsApi(config);

  const body = {
    // string | Tunnel identifier
    id: id_example,
  } satisfies RevokeTunnelRequest;

  try {
    const data = await api.revokeTunnel(body);
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
| **id** | `string` | Tunnel identifier | [Defaults to `undefined`] |

### Return type

[**SuccessResponse**](SuccessResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Tunnel revoked |  -  |
| **401** | Unauthorized |  -  |
| **404** | Tunnel not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)

