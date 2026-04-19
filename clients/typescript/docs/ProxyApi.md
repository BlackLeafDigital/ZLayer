# ProxyApi

All URIs are relative to *http://localhost*

| Method | HTTP request | Description |
|------------- | ------------- | -------------|
| [**listBackends**](ProxyApi.md#listbackends) | **GET** /api/v1/proxy/backends | List all load-balancer backend groups. |
| [**listRoutes**](ProxyApi.md#listroutes) | **GET** /api/v1/proxy/routes | List all registered proxy routes. |
| [**listStreams**](ProxyApi.md#liststreams) | **GET** /api/v1/proxy/streams | List L4 stream proxies. |
| [**listTls**](ProxyApi.md#listtls) | **GET** /api/v1/proxy/tls | List loaded TLS certificates. |



## listBackends

> BackendsResponse listBackends()

List all load-balancer backend groups.

Returns each service\&#39;s backend group with its strategy, health status, and active connection counts.  # Errors  Returns &#x60;ApiError::ServiceUnavailable&#x60; if the load balancer is not initialised.

### Example

```ts
import {
  Configuration,
  ProxyApi,
} from '@zlayer/client';
import type { ListBackendsRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new ProxyApi(config);

  try {
    const data = await api.listBackends();
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

[**BackendsResponse**](BackendsResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Load-balancer backend groups |  -  |
| **503** | Load balancer not available |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## listRoutes

> RoutesResponse listRoutes()

List all registered proxy routes.

Returns the full list of L7 routes with their host patterns, path prefixes, backends, and protocol information.  # Errors  Returns &#x60;ApiError::ServiceUnavailable&#x60; if the service registry is not initialised.

### Example

```ts
import {
  Configuration,
  ProxyApi,
} from '@zlayer/client';
import type { ListRoutesRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new ProxyApi(config);

  try {
    const data = await api.listRoutes();
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

[**RoutesResponse**](RoutesResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | List of routes |  -  |
| **503** | Service registry not available |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## listStreams

> StreamsResponse listStreams()

List L4 stream proxies.

Returns all TCP and UDP stream proxies with their listen ports, service names, and backends.  # Errors  Returns &#x60;ApiError::ServiceUnavailable&#x60; if the stream registry is not initialised.

### Example

```ts
import {
  Configuration,
  ProxyApi,
} from '@zlayer/client';
import type { ListStreamsRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new ProxyApi(config);

  try {
    const data = await api.listStreams();
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

[**StreamsResponse**](StreamsResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | L4 stream proxies |  -  |
| **503** | Stream registry not available |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## listTls

> TlsResponse listTls()

List loaded TLS certificates.

Returns cached certificate domains with metadata (expiry, fingerprint) when available.  # Errors  Returns &#x60;ApiError::ServiceUnavailable&#x60; if the certificate manager is not initialised.

### Example

```ts
import {
  Configuration,
  ProxyApi,
} from '@zlayer/client';
import type { ListTlsRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new ProxyApi(config);

  try {
    const data = await api.listTls();
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

[**TlsResponse**](TlsResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Loaded TLS certificates |  -  |
| **503** | Certificate manager not available |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)

