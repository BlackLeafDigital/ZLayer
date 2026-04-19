# EnvironmentsApi

All URIs are relative to *http://localhost*

| Method | HTTP request | Description |
|------------- | ------------- | -------------|
| [**createEnvironment**](EnvironmentsApi.md#createenvironmentoperation) | **POST** /api/v1/environments | Create a new environment. Admin only. |
| [**deleteEnvironment**](EnvironmentsApi.md#deleteenvironment) | **DELETE** /api/v1/environments/{id} | Delete an environment. Admin only. |
| [**getEnvironment**](EnvironmentsApi.md#getenvironment) | **GET** /api/v1/environments/{id} | Fetch a single environment by id. |
| [**listEnvironments**](EnvironmentsApi.md#listenvironments) | **GET** /api/v1/environments | List environments. |
| [**updateEnvironment**](EnvironmentsApi.md#updateenvironmentoperation) | **PATCH** /api/v1/environments/{id} | Rename / re-describe an environment. Admin only. |



## createEnvironment

> StoredEnvironment createEnvironment(createEnvironmentRequest)

Create a new environment. Admin only.

# Errors  Returns [&#x60;ApiError::Forbidden&#x60;] when the caller is not an admin, [&#x60;ApiError::BadRequest&#x60;] for an empty name, [&#x60;ApiError::Conflict&#x60;] when the &#x60;(name, project_id)&#x60; is already used, or [&#x60;ApiError::Internal&#x60;] when the environment store fails.

### Example

```ts
import {
  Configuration,
  EnvironmentsApi,
} from '@zlayer/client';
import type { CreateEnvironmentOperationRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new EnvironmentsApi(config);

  const body = {
    // CreateEnvironmentRequest
    createEnvironmentRequest: ...,
  } satisfies CreateEnvironmentOperationRequest;

  try {
    const data = await api.createEnvironment(body);
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
| **createEnvironmentRequest** | [CreateEnvironmentRequest](CreateEnvironmentRequest.md) |  | |

### Return type

[**StoredEnvironment**](StoredEnvironment.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: `application/json`
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **201** | Environment created |  -  |
| **400** | Invalid request |  -  |
| **403** | Admin role required |  -  |
| **409** | Name already used in the given project |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## deleteEnvironment

> deleteEnvironment(id)

Delete an environment. Admin only.

Non-cascading: the operator must purge any secrets attached to the environment before deletion (typically via &#x60;DELETE /api/v1/secrets?env&#x3D;{id}&amp;all&#x3D;true&#x60;). If the environment still has secrets, this endpoint returns 409.  # Errors  Returns [&#x60;ApiError::Forbidden&#x60;] for non-admins, [&#x60;ApiError::NotFound&#x60;] when the environment does not exist, [&#x60;ApiError::Conflict&#x60;] when secrets remain, or [&#x60;ApiError::Internal&#x60;] when a backing store fails.

### Example

```ts
import {
  Configuration,
  EnvironmentsApi,
} from '@zlayer/client';
import type { DeleteEnvironmentRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new EnvironmentsApi(config);

  const body = {
    // string | Environment id
    id: id_example,
  } satisfies DeleteEnvironmentRequest;

  try {
    const data = await api.deleteEnvironment(body);
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
| **id** | `string` | Environment id | [Defaults to `undefined`] |

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
| **204** | Environment deleted |  -  |
| **403** | Admin role required |  -  |
| **404** | Not found |  -  |
| **409** | Environment still has secrets |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## getEnvironment

> StoredEnvironment getEnvironment(id)

Fetch a single environment by id.

# Errors  Returns [&#x60;ApiError::NotFound&#x60;] if no environment with the given id exists, or [&#x60;ApiError::Internal&#x60;] when the store fails.

### Example

```ts
import {
  Configuration,
  EnvironmentsApi,
} from '@zlayer/client';
import type { GetEnvironmentRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new EnvironmentsApi(config);

  const body = {
    // string | Environment id
    id: id_example,
  } satisfies GetEnvironmentRequest;

  try {
    const data = await api.getEnvironment(body);
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
| **id** | `string` | Environment id | [Defaults to `undefined`] |

### Return type

[**StoredEnvironment**](StoredEnvironment.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Environment |  -  |
| **404** | Not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## listEnvironments

> Array&lt;StoredEnvironment&gt; listEnvironments(project)

List environments.

Filter rules (see [&#x60;ListEnvironmentsQuery&#x60;]):  - &#x60;?project&#x3D;{id}&#x60; → environments belonging to that project. - &#x60;?project&#x3D;*&#x60; → every environment across every project + globals,   merged and sorted by name. - no &#x60;project&#x60; query → only global environments (&#x60;project_id IS NULL&#x60;).  # Errors  Returns [&#x60;ApiError::Internal&#x60;] if the environment store fails.

### Example

```ts
import {
  Configuration,
  EnvironmentsApi,
} from '@zlayer/client';
import type { ListEnvironmentsRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new EnvironmentsApi(config);

  const body = {
    // string | Project id to filter by; \'*\' lists all; omit for globals only (optional)
    project: project_example,
  } satisfies ListEnvironmentsRequest;

  try {
    const data = await api.listEnvironments(body);
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
| **project** | `string` | Project id to filter by; \&#39;*\&#39; lists all; omit for globals only | [Optional] [Defaults to `undefined`] |

### Return type

[**Array&lt;StoredEnvironment&gt;**](StoredEnvironment.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | List of environments |  -  |
| **401** | Unauthorized |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## updateEnvironment

> StoredEnvironment updateEnvironment(id, updateEnvironmentRequest)

Rename / re-describe an environment. Admin only.

# Errors  Returns [&#x60;ApiError::Forbidden&#x60;] for non-admins, [&#x60;ApiError::NotFound&#x60;] when the environment does not exist, [&#x60;ApiError::BadRequest&#x60;] for an empty replacement name, [&#x60;ApiError::Conflict&#x60;] when the new name collides with another environment in the same scope, or [&#x60;ApiError::Internal&#x60;] when the store fails.

### Example

```ts
import {
  Configuration,
  EnvironmentsApi,
} from '@zlayer/client';
import type { UpdateEnvironmentOperationRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new EnvironmentsApi(config);

  const body = {
    // string | Environment id
    id: id_example,
    // UpdateEnvironmentRequest
    updateEnvironmentRequest: ...,
  } satisfies UpdateEnvironmentOperationRequest;

  try {
    const data = await api.updateEnvironment(body);
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
| **id** | `string` | Environment id | [Defaults to `undefined`] |
| **updateEnvironmentRequest** | [UpdateEnvironmentRequest](UpdateEnvironmentRequest.md) |  | |

### Return type

[**StoredEnvironment**](StoredEnvironment.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: `application/json`
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Updated environment |  -  |
| **400** | Invalid request |  -  |
| **403** | Admin role required |  -  |
| **404** | Not found |  -  |
| **409** | Name collides with another environment |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)

