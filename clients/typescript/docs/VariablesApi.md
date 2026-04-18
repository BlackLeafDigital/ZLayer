# VariablesApi

All URIs are relative to *http://localhost*

| Method | HTTP request | Description |
|------------- | ------------- | -------------|
| [**createVariable**](VariablesApi.md#createvariableoperation) | **POST** /api/v1/variables | Create a new variable. Admin only. |
| [**deleteVariable**](VariablesApi.md#deletevariable) | **DELETE** /api/v1/variables/{id} | Delete a variable. Admin only. |
| [**getVariable**](VariablesApi.md#getvariable) | **GET** /api/v1/variables/{id} | Fetch a single variable by id. |
| [**listVariables**](VariablesApi.md#listvariables) | **GET** /api/v1/variables | List variables. |
| [**updateVariable**](VariablesApi.md#updatevariableoperation) | **PATCH** /api/v1/variables/{id} | Update a variable\&#39;s name and/or value. Admin only. |



## createVariable

> StoredVariable createVariable(createVariableRequest)

Create a new variable. Admin only.

# Errors  Returns [&#x60;ApiError::Forbidden&#x60;] when the caller is not an admin, [&#x60;ApiError::BadRequest&#x60;] for an empty name, [&#x60;ApiError::Conflict&#x60;] when the &#x60;(name, scope)&#x60; is already used, or [&#x60;ApiError::Internal&#x60;] when the variable store fails.

### Example

```ts
import {
  Configuration,
  VariablesApi,
} from '@zlayer/client';
import type { CreateVariableOperationRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new VariablesApi(config);

  const body = {
    // CreateVariableRequest
    createVariableRequest: ...,
  } satisfies CreateVariableOperationRequest;

  try {
    const data = await api.createVariable(body);
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
| **createVariableRequest** | [CreateVariableRequest](CreateVariableRequest.md) |  | |

### Return type

[**StoredVariable**](StoredVariable.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: `application/json`
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **201** | Variable created |  -  |
| **400** | Invalid request |  -  |
| **403** | Admin role required |  -  |
| **409** | Name already used in the given scope |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## deleteVariable

> deleteVariable(id)

Delete a variable. Admin only.

# Errors  Returns [&#x60;ApiError::Forbidden&#x60;] for non-admins, [&#x60;ApiError::NotFound&#x60;] when the variable does not exist, or [&#x60;ApiError::Internal&#x60;] when the store fails.

### Example

```ts
import {
  Configuration,
  VariablesApi,
} from '@zlayer/client';
import type { DeleteVariableRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new VariablesApi(config);

  const body = {
    // string | Variable id
    id: id_example,
  } satisfies DeleteVariableRequest;

  try {
    const data = await api.deleteVariable(body);
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
| **id** | `string` | Variable id | [Defaults to `undefined`] |

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
| **204** | Variable deleted |  -  |
| **403** | Admin role required |  -  |
| **404** | Not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## getVariable

> StoredVariable getVariable(id)

Fetch a single variable by id.

# Errors  Returns [&#x60;ApiError::NotFound&#x60;] if no variable with the given id exists, or [&#x60;ApiError::Internal&#x60;] when the store fails.

### Example

```ts
import {
  Configuration,
  VariablesApi,
} from '@zlayer/client';
import type { GetVariableRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new VariablesApi(config);

  const body = {
    // string | Variable id
    id: id_example,
  } satisfies GetVariableRequest;

  try {
    const data = await api.getVariable(body);
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
| **id** | `string` | Variable id | [Defaults to `undefined`] |

### Return type

[**StoredVariable**](StoredVariable.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Variable |  -  |
| **404** | Not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## listVariables

> Array&lt;StoredVariable&gt; listVariables(scope)

List variables.

Filter rules (see [&#x60;ListVariablesQuery&#x60;]):  - &#x60;?scope&#x3D;{id}&#x60; -&gt; variables belonging to that scope. - no &#x60;scope&#x60; query -&gt; only global variables (&#x60;scope IS NULL&#x60;).  # Errors  Returns [&#x60;ApiError::Internal&#x60;] if the variable store fails.

### Example

```ts
import {
  Configuration,
  VariablesApi,
} from '@zlayer/client';
import type { ListVariablesRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new VariablesApi(config);

  const body = {
    // string | Scope (project id) to filter by; omit for globals only (optional)
    scope: scope_example,
  } satisfies ListVariablesRequest;

  try {
    const data = await api.listVariables(body);
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
| **scope** | `string` | Scope (project id) to filter by; omit for globals only | [Optional] [Defaults to `undefined`] |

### Return type

[**Array&lt;StoredVariable&gt;**](StoredVariable.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | List of variables |  -  |
| **401** | Unauthorized |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## updateVariable

> StoredVariable updateVariable(id, updateVariableRequest)

Update a variable\&#39;s name and/or value. Admin only.

# Errors  Returns [&#x60;ApiError::Forbidden&#x60;] for non-admins, [&#x60;ApiError::NotFound&#x60;] when the variable does not exist, [&#x60;ApiError::BadRequest&#x60;] for an empty replacement name, [&#x60;ApiError::Conflict&#x60;] when the new name collides with another variable in the same scope, or [&#x60;ApiError::Internal&#x60;] when the store fails.

### Example

```ts
import {
  Configuration,
  VariablesApi,
} from '@zlayer/client';
import type { UpdateVariableOperationRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new VariablesApi(config);

  const body = {
    // string | Variable id
    id: id_example,
    // UpdateVariableRequest
    updateVariableRequest: ...,
  } satisfies UpdateVariableOperationRequest;

  try {
    const data = await api.updateVariable(body);
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
| **id** | `string` | Variable id | [Defaults to `undefined`] |
| **updateVariableRequest** | [UpdateVariableRequest](UpdateVariableRequest.md) |  | |

### Return type

[**StoredVariable**](StoredVariable.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: `application/json`
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Updated variable |  -  |
| **400** | Invalid request |  -  |
| **403** | Admin role required |  -  |
| **404** | Not found |  -  |
| **409** | Name collides with another variable |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)

