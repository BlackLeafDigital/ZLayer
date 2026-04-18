# ContainersApi

All URIs are relative to *http://localhost*

| Method | HTTP request | Description |
|------------- | ------------- | -------------|
| [**createContainer**](ContainersApi.md#createcontaineroperation) | **POST** /api/v1/containers | Create and start a container. |
| [**deleteContainer**](ContainersApi.md#deletecontainer) | **DELETE** /api/v1/containers/{id} | Stop and remove a container. |
| [**execInContainer**](ContainersApi.md#execincontainer) | **POST** /api/v1/containers/{id}/exec | Execute a command in a running container. |
| [**getContainer**](ContainersApi.md#getcontainer) | **GET** /api/v1/containers/{id} | Get details for a specific container. |
| [**getContainerLogs**](ContainersApi.md#getcontainerlogs) | **GET** /api/v1/containers/{id}/logs | Get container logs. |
| [**getContainerStats**](ContainersApi.md#getcontainerstats) | **GET** /api/v1/containers/{id}/stats | Get container resource statistics. |
| [**killContainer**](ContainersApi.md#killcontaineroperation) | **POST** /api/v1/containers/{id}/kill | Send a signal to a running container. |
| [**listContainers**](ContainersApi.md#listcontainers) | **GET** /api/v1/containers | List standalone containers. |
| [**restartContainer**](ContainersApi.md#restartcontaineroperation) | **POST** /api/v1/containers/{id}/restart | Restart a container: stop then start. |
| [**startContainer**](ContainersApi.md#startcontainer) | **POST** /api/v1/containers/{id}/start | Start a previously-created container. |
| [**stopContainer**](ContainersApi.md#stopcontaineroperation) | **POST** /api/v1/containers/{id}/stop | Stop a running container. |
| [**waitContainer**](ContainersApi.md#waitcontainer) | **GET** /api/v1/containers/{id}/wait | Wait for a container to exit and return its exit code. |



## createContainer

> ContainerInfo createContainer(createContainerRequest)

Create and start a container.

Pulls the image if needed, creates the container, and starts it. Returns the container info including its assigned ID.  # Errors  Returns an error if image pull fails, container creation fails, or the user lacks the operator role.

### Example

```ts
import {
  Configuration,
  ContainersApi,
} from '@zlayer/client';
import type { CreateContainerOperationRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new ContainersApi(config);

  const body = {
    // CreateContainerRequest
    createContainerRequest: ...,
  } satisfies CreateContainerOperationRequest;

  try {
    const data = await api.createContainer(body);
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
| **createContainerRequest** | [CreateContainerRequest](CreateContainerRequest.md) |  | |

### Return type

[**ContainerInfo**](ContainerInfo.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: `application/json`
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **201** | Container created and started |  -  |
| **400** | Invalid request |  -  |
| **401** | Unauthorized |  -  |
| **403** | Forbidden - operator role required |  -  |
| **500** | Internal error |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## deleteContainer

> deleteContainer(id)

Stop and remove a container.

Sends a stop signal (with a 30-second timeout), then removes the container.  # Errors  Returns an error if the container is not found, stop/remove fails, or the user lacks the operator role.

### Example

```ts
import {
  Configuration,
  ContainersApi,
} from '@zlayer/client';
import type { DeleteContainerRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new ContainersApi(config);

  const body = {
    // string | Container identifier
    id: id_example,
  } satisfies DeleteContainerRequest;

  try {
    const data = await api.deleteContainer(body);
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
| **id** | `string` | Container identifier | [Defaults to `undefined`] |

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
| **204** | Container stopped and removed |  -  |
| **401** | Unauthorized |  -  |
| **403** | Forbidden - operator role required |  -  |
| **404** | Container not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## execInContainer

> ContainerExecResponse execInContainer(id, containerExecRequest)

Execute a command in a running container.

# Errors  Returns an error if the container is not found, the command is invalid, execution fails, or the user lacks the operator role.

### Example

```ts
import {
  Configuration,
  ContainersApi,
} from '@zlayer/client';
import type { ExecInContainerRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new ContainersApi(config);

  const body = {
    // string | Container identifier
    id: id_example,
    // ContainerExecRequest
    containerExecRequest: ...,
  } satisfies ExecInContainerRequest;

  try {
    const data = await api.execInContainer(body);
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
| **id** | `string` | Container identifier | [Defaults to `undefined`] |
| **containerExecRequest** | [ContainerExecRequest](ContainerExecRequest.md) |  | |

### Return type

[**ContainerExecResponse**](ContainerExecResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: `application/json`
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Command executed |  -  |
| **400** | Invalid request |  -  |
| **401** | Unauthorized |  -  |
| **403** | Forbidden - operator role required |  -  |
| **404** | Container not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## getContainer

> ContainerInfo getContainer(id)

Get details for a specific container.

# Errors  Returns an error if the container is not found or the user is not authenticated.

### Example

```ts
import {
  Configuration,
  ContainersApi,
} from '@zlayer/client';
import type { GetContainerRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new ContainersApi(config);

  const body = {
    // string | Container identifier
    id: id_example,
  } satisfies GetContainerRequest;

  try {
    const data = await api.getContainer(body);
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
| **id** | `string` | Container identifier | [Defaults to `undefined`] |

### Return type

[**ContainerInfo**](ContainerInfo.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Container details |  -  |
| **401** | Unauthorized |  -  |
| **404** | Container not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## getContainerLogs

> string getContainerLogs(id, tail, follow)

Get container logs.

Returns the last N lines of container logs as plain text, or streams logs as Server-Sent Events when &#x60;follow&#x3D;true&#x60;.  # Errors  Returns an error if the container is not found or log retrieval fails.

### Example

```ts
import {
  Configuration,
  ContainersApi,
} from '@zlayer/client';
import type { GetContainerLogsRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new ContainersApi(config);

  const body = {
    // string | Container identifier
    id: id_example,
    // number | Number of tail lines to return (optional)
    tail: 56,
    // boolean | Follow logs (SSE stream) (optional)
    follow: true,
  } satisfies GetContainerLogsRequest;

  try {
    const data = await api.getContainerLogs(body);
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
| **id** | `string` | Container identifier | [Defaults to `undefined`] |
| **tail** | `number` | Number of tail lines to return | [Optional] [Defaults to `undefined`] |
| **follow** | `boolean` | Follow logs (SSE stream) | [Optional] [Defaults to `undefined`] |

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
| **200** | Container logs (plain text or SSE stream) |  -  |
| **401** | Unauthorized |  -  |
| **404** | Container not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## getContainerStats

> ContainerStatsResponse getContainerStats(id)

Get container resource statistics.

Returns CPU and memory usage statistics for the specified container.  # Errors  Returns an error if the container is not found or stats retrieval fails.

### Example

```ts
import {
  Configuration,
  ContainersApi,
} from '@zlayer/client';
import type { GetContainerStatsRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new ContainersApi(config);

  const body = {
    // string | Container identifier
    id: id_example,
  } satisfies GetContainerStatsRequest;

  try {
    const data = await api.getContainerStats(body);
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
| **id** | `string` | Container identifier | [Defaults to `undefined`] |

### Return type

[**ContainerStatsResponse**](ContainerStatsResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Container statistics |  -  |
| **401** | Unauthorized |  -  |
| **404** | Container not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## killContainer

> killContainer(id, killContainerRequest)

Send a signal to a running container.

Mirrors Docker-compat &#x60;POST /containers/{id}/kill&#x60;. When the request body\&#39;s &#x60;signal&#x60; field is omitted, the runtime sends &#x60;SIGKILL&#x60;. Accepted signals are &#x60;SIGKILL&#x60;, &#x60;SIGTERM&#x60;, &#x60;SIGINT&#x60;, &#x60;SIGHUP&#x60;, &#x60;SIGUSR1&#x60;, &#x60;SIGUSR2&#x60; (with or without the &#x60;SIG&#x60; prefix); any other value is rejected with &#x60;400&#x60;.  # Errors  Returns &#x60;400&#x60; for unknown signals, &#x60;404&#x60; if the container is not found, &#x60;403&#x60; if the caller lacks the &#x60;operator&#x60; role, &#x60;501&#x60; if the runtime does not support &#x60;kill_container&#x60;, and &#x60;500&#x60; for other runtime errors.

### Example

```ts
import {
  Configuration,
  ContainersApi,
} from '@zlayer/client';
import type { KillContainerOperationRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new ContainersApi(config);

  const body = {
    // string | Container identifier
    id: id_example,
    // KillContainerRequest
    killContainerRequest: ...,
  } satisfies KillContainerOperationRequest;

  try {
    const data = await api.killContainer(body);
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
| **id** | `string` | Container identifier | [Defaults to `undefined`] |
| **killContainerRequest** | [KillContainerRequest](KillContainerRequest.md) |  | |

### Return type

`void` (Empty response body)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: `application/json`
- **Accept**: Not defined


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **204** | Signal delivered |  -  |
| **400** | Invalid signal |  -  |
| **401** | Unauthorized |  -  |
| **403** | Forbidden - operator role required |  -  |
| **404** | Container not found |  -  |
| **500** | Internal error |  -  |
| **501** | Runtime does not support kill |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## listContainers

> Array&lt;ContainerInfo&gt; listContainers(label)

List standalone containers.

Returns all containers managed through this API. Optionally filter by label using the &#x60;label&#x60; query parameter in &#x60;key&#x3D;value&#x60; format.  # Errors  Returns an error if the user is not authenticated.

### Example

```ts
import {
  Configuration,
  ContainersApi,
} from '@zlayer/client';
import type { ListContainersRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new ContainersApi(config);

  const body = {
    // string | Filter by label (key=value format) (optional)
    label: label_example,
  } satisfies ListContainersRequest;

  try {
    const data = await api.listContainers(body);
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
| **label** | `string` | Filter by label (key&#x3D;value format) | [Optional] [Defaults to `undefined`] |

### Return type

[**Array&lt;ContainerInfo&gt;**](ContainerInfo.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | List of containers |  -  |
| **401** | Unauthorized |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## restartContainer

> restartContainer(id, restartContainerRequest)

Restart a container: stop then start.

Composes &#x60;stop_container(timeout)&#x60; followed by &#x60;start_container&#x60;. Errors during the stop phase are ignored (the container may already be stopped); errors during the start phase are surfaced.  # Errors  Returns an error if the container is not found, the start phase fails, or the user lacks the operator role.

### Example

```ts
import {
  Configuration,
  ContainersApi,
} from '@zlayer/client';
import type { RestartContainerOperationRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new ContainersApi(config);

  const body = {
    // string | Container identifier
    id: id_example,
    // RestartContainerRequest
    restartContainerRequest: ...,
  } satisfies RestartContainerOperationRequest;

  try {
    const data = await api.restartContainer(body);
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
| **id** | `string` | Container identifier | [Defaults to `undefined`] |
| **restartContainerRequest** | [RestartContainerRequest](RestartContainerRequest.md) |  | |

### Return type

`void` (Empty response body)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: `application/json`
- **Accept**: Not defined


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **204** | Container restarted |  -  |
| **401** | Unauthorized |  -  |
| **403** | Forbidden - operator role required |  -  |
| **404** | Container not found |  -  |
| **500** | Internal error |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## startContainer

> startContainer(id)

Start a previously-created container.

Useful for re-starting a container that was stopped via &#x60;POST /api/v1/containers/{id}/stop&#x60; without being removed.  # Errors  Returns an error if the container is not found, start fails, or the user lacks the operator role.

### Example

```ts
import {
  Configuration,
  ContainersApi,
} from '@zlayer/client';
import type { StartContainerRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new ContainersApi(config);

  const body = {
    // string | Container identifier
    id: id_example,
  } satisfies StartContainerRequest;

  try {
    const data = await api.startContainer(body);
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
| **id** | `string` | Container identifier | [Defaults to `undefined`] |

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
| **204** | Container started |  -  |
| **401** | Unauthorized |  -  |
| **403** | Forbidden - operator role required |  -  |
| **404** | Container not found |  -  |
| **500** | Internal error |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## stopContainer

> stopContainer(id, stopContainerRequest)

Stop a running container.

Sends the runtime\&#39;s graceful stop signal and waits up to &#x60;timeout&#x60; seconds before force-killing. The container is **not** removed; use &#x60;DELETE /api/v1/containers/{id}&#x60; to stop-and-remove. Idempotent: calling stop on an already-stopped container is not an error.  # Errors  Returns an error if the container is not found, stop fails, or the user lacks the operator role.

### Example

```ts
import {
  Configuration,
  ContainersApi,
} from '@zlayer/client';
import type { StopContainerOperationRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new ContainersApi(config);

  const body = {
    // string | Container identifier
    id: id_example,
    // StopContainerRequest
    stopContainerRequest: ...,
  } satisfies StopContainerOperationRequest;

  try {
    const data = await api.stopContainer(body);
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
| **id** | `string` | Container identifier | [Defaults to `undefined`] |
| **stopContainerRequest** | [StopContainerRequest](StopContainerRequest.md) |  | |

### Return type

`void` (Empty response body)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: `application/json`
- **Accept**: Not defined


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **204** | Container stopped |  -  |
| **401** | Unauthorized |  -  |
| **403** | Forbidden - operator role required |  -  |
| **404** | Container not found |  -  |
| **500** | Internal error |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## waitContainer

> ContainerWaitResponse waitContainer(id)

Wait for a container to exit and return its exit code.

This endpoint blocks until the container exits. Useful for CI runners that need to wait for a build/test container to complete.  # Errors  Returns an error if the container is not found or the wait fails.

### Example

```ts
import {
  Configuration,
  ContainersApi,
} from '@zlayer/client';
import type { WaitContainerRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new ContainersApi(config);

  const body = {
    // string | Container identifier
    id: id_example,
  } satisfies WaitContainerRequest;

  try {
    const data = await api.waitContainer(body);
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
| **id** | `string` | Container identifier | [Defaults to `undefined`] |

### Return type

[**ContainerWaitResponse**](ContainerWaitResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Container exited |  -  |
| **401** | Unauthorized |  -  |
| **404** | Container not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)

