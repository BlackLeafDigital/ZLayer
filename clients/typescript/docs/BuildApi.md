# BuildApi

All URIs are relative to *http://localhost*

| Method | HTTP request | Description |
|------------- | ------------- | -------------|
| [**getBuildLogs**](BuildApi.md#getbuildlogs) | **GET** /api/v1/build/{id}/logs | GET /api/v1/build/{id}/logs Get build logs. |
| [**getBuildStatus**](BuildApi.md#getbuildstatus) | **GET** /api/v1/build/{id} | GET /api/v1/build/{id} Get build status. |
| [**listBuilds**](BuildApi.md#listbuilds) | **GET** /api/v1/builds | GET /api/v1/builds List all builds. |
| [**listRuntimeTemplates**](BuildApi.md#listruntimetemplates) | **GET** /api/v1/templates | GET /api/v1/templates List available runtime templates |
| [**startBuild**](BuildApi.md#startbuild) | **POST** /api/v1/build | POST /api/v1/build Start a new build from multipart upload (Dockerfile + context tarball) |
| [**startBuildJson**](BuildApi.md#startbuildjson) | **POST** /api/v1/build/json | POST /api/v1/build/json Start a new build from JSON request with a context path on the server. |
| [**streamBuild**](BuildApi.md#streambuild) | **GET** /api/v1/build/{id}/stream | GET /api/v1/build/{id}/stream Stream build progress via Server-Sent Events. |



## getBuildLogs

> string getBuildLogs(id)

GET /api/v1/build/{id}/logs Get build logs.

# Errors  Returns an error if the build is not found or logs cannot be read.

### Example

```ts
import {
  Configuration,
  BuildApi,
} from '@zlayer/client';
import type { GetBuildLogsRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new BuildApi(config);

  const body = {
    // string | Build ID
    id: id_example,
  } satisfies GetBuildLogsRequest;

  try {
    const data = await api.getBuildLogs(body);
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
| **id** | `string` | Build ID | [Defaults to `undefined`] |

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
| **200** | Build logs |  -  |
| **401** | Unauthorized |  -  |
| **404** | Build not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## getBuildStatus

> BuildStatus getBuildStatus(id)

GET /api/v1/build/{id} Get build status.

# Errors  Returns an error if the build is not found.

### Example

```ts
import {
  Configuration,
  BuildApi,
} from '@zlayer/client';
import type { GetBuildStatusRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new BuildApi(config);

  const body = {
    // string | Build ID
    id: id_example,
  } satisfies GetBuildStatusRequest;

  try {
    const data = await api.getBuildStatus(body);
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
| **id** | `string` | Build ID | [Defaults to `undefined`] |

### Return type

[**BuildStatus**](BuildStatus.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Build status |  -  |
| **401** | Unauthorized |  -  |
| **404** | Build not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## listBuilds

> Array&lt;BuildStatus&gt; listBuilds()

GET /api/v1/builds List all builds.

# Errors  Returns an error if the user is not authenticated.

### Example

```ts
import {
  Configuration,
  BuildApi,
} from '@zlayer/client';
import type { ListBuildsRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new BuildApi(config);

  try {
    const data = await api.listBuilds();
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

[**Array&lt;BuildStatus&gt;**](BuildStatus.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | List of builds |  -  |
| **401** | Unauthorized |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## listRuntimeTemplates

> Array&lt;TemplateInfo&gt; listRuntimeTemplates()

GET /api/v1/templates List available runtime templates

### Example

```ts
import {
  Configuration,
  BuildApi,
} from '@zlayer/client';
import type { ListRuntimeTemplatesRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const api = new BuildApi();

  try {
    const data = await api.listRuntimeTemplates();
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

[**Array&lt;TemplateInfo&gt;**](TemplateInfo.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | List of templates |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## startBuild

> TriggerBuildResponse startBuild()

POST /api/v1/build Start a new build from multipart upload (Dockerfile + context tarball)

Accepts a multipart form with: - &#x60;dockerfile&#x60;: The Dockerfile content (optional if using runtime) - &#x60;context&#x60;: A tarball containing the build context - &#x60;config&#x60;: JSON configuration (&#x60;BuildRequest&#x60;)  # Errors  Returns an error if the request is invalid, context extraction fails, or the build cannot be started.

### Example

```ts
import {
  Configuration,
  BuildApi,
} from '@zlayer/client';
import type { StartBuildRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new BuildApi(config);

  try {
    const data = await api.startBuild();
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

[**TriggerBuildResponse**](TriggerBuildResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: `multipart/form-data`
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **202** | Build started |  -  |
| **400** | Invalid request |  -  |
| **401** | Unauthorized |  -  |
| **500** | Internal error |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## startBuildJson

> TriggerBuildResponse startBuildJson(buildRequestWithContext)

POST /api/v1/build/json Start a new build from JSON request with a context path on the server.

# Errors  Returns an error if the context path is invalid or the build cannot be started.

### Example

```ts
import {
  Configuration,
  BuildApi,
} from '@zlayer/client';
import type { StartBuildJsonRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new BuildApi(config);

  const body = {
    // BuildRequestWithContext
    buildRequestWithContext: ...,
  } satisfies StartBuildJsonRequest;

  try {
    const data = await api.startBuildJson(body);
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
| **buildRequestWithContext** | [BuildRequestWithContext](BuildRequestWithContext.md) |  | |

### Return type

[**TriggerBuildResponse**](TriggerBuildResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: `application/json`
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **202** | Build started |  -  |
| **400** | Invalid request |  -  |
| **401** | Unauthorized |  -  |
| **500** | Internal error |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## streamBuild

> streamBuild(id)

GET /api/v1/build/{id}/stream Stream build progress via Server-Sent Events.

# Errors  Returns an error if the build is not found or not active.

### Example

```ts
import {
  Configuration,
  BuildApi,
} from '@zlayer/client';
import type { StreamBuildRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new BuildApi(config);

  const body = {
    // string | Build ID
    id: id_example,
  } satisfies StreamBuildRequest;

  try {
    const data = await api.streamBuild(body);
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
| **id** | `string` | Build ID | [Defaults to `undefined`] |

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
| **200** | SSE event stream |  -  |
| **401** | Unauthorized |  -  |
| **404** | Build not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)

