# CredentialsApi

All URIs are relative to *http://localhost*

| Method | HTTP request | Description |
|------------- | ------------- | -------------|
| [**createGitCredential**](CredentialsApi.md#creategitcredentialoperation) | **POST** /api/v1/credentials/git | Create a new git credential. Admin only. |
| [**createRegistryCredential**](CredentialsApi.md#createregistrycredentialoperation) | **POST** /api/v1/credentials/registry | Create a new registry credential. Admin only. |
| [**deleteGitCredential**](CredentialsApi.md#deletegitcredential) | **DELETE** /api/v1/credentials/git/{id} | Delete a git credential. Admin only. |
| [**deleteRegistryCredential**](CredentialsApi.md#deleteregistrycredential) | **DELETE** /api/v1/credentials/registry/{id} | Delete a registry credential. Admin only. |
| [**listGitCredentials**](CredentialsApi.md#listgitcredentials) | **GET** /api/v1/credentials/git | List all git credentials (metadata only, no secret values). |
| [**listRegistryCredentials**](CredentialsApi.md#listregistrycredentials) | **GET** /api/v1/credentials/registry | List all registry credentials (metadata only, no passwords). |



## createGitCredential

> GitCredentialResponse createGitCredential(createGitCredentialRequest)

Create a new git credential. Admin only.

# Errors  Returns [&#x60;ApiError::Forbidden&#x60;] for non-admins, [&#x60;ApiError::BadRequest&#x60;] for missing fields, or [&#x60;ApiError::Internal&#x60;] when the store fails.

### Example

```ts
import {
  Configuration,
  CredentialsApi,
} from '@zlayer/client';
import type { CreateGitCredentialOperationRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new CredentialsApi(config);

  const body = {
    // CreateGitCredentialRequest
    createGitCredentialRequest: ...,
  } satisfies CreateGitCredentialOperationRequest;

  try {
    const data = await api.createGitCredential(body);
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
| **createGitCredentialRequest** | [CreateGitCredentialRequest](CreateGitCredentialRequest.md) |  | |

### Return type

[**GitCredentialResponse**](GitCredentialResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: `application/json`
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **201** | Git credential created |  -  |
| **400** | Invalid request |  -  |
| **403** | Admin role required |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## createRegistryCredential

> RegistryCredentialResponse createRegistryCredential(createRegistryCredentialRequest)

Create a new registry credential. Admin only.

# Errors  Returns [&#x60;ApiError::Forbidden&#x60;] for non-admins, [&#x60;ApiError::BadRequest&#x60;] for missing fields, or [&#x60;ApiError::Internal&#x60;] when the store fails.

### Example

```ts
import {
  Configuration,
  CredentialsApi,
} from '@zlayer/client';
import type { CreateRegistryCredentialOperationRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new CredentialsApi(config);

  const body = {
    // CreateRegistryCredentialRequest
    createRegistryCredentialRequest: ...,
  } satisfies CreateRegistryCredentialOperationRequest;

  try {
    const data = await api.createRegistryCredential(body);
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
| **createRegistryCredentialRequest** | [CreateRegistryCredentialRequest](CreateRegistryCredentialRequest.md) |  | |

### Return type

[**RegistryCredentialResponse**](RegistryCredentialResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: `application/json`
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **201** | Registry credential created |  -  |
| **400** | Invalid request |  -  |
| **403** | Admin role required |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## deleteGitCredential

> deleteGitCredential(id)

Delete a git credential. Admin only.

# Errors  Returns [&#x60;ApiError::Forbidden&#x60;] for non-admins, [&#x60;ApiError::NotFound&#x60;] when the credential does not exist, or [&#x60;ApiError::Internal&#x60;] when the store fails.

### Example

```ts
import {
  Configuration,
  CredentialsApi,
} from '@zlayer/client';
import type { DeleteGitCredentialRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new CredentialsApi(config);

  const body = {
    // string | Git credential id
    id: id_example,
  } satisfies DeleteGitCredentialRequest;

  try {
    const data = await api.deleteGitCredential(body);
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
| **id** | `string` | Git credential id | [Defaults to `undefined`] |

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
| **204** | Git credential deleted |  -  |
| **403** | Admin role required |  -  |
| **404** | Not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## deleteRegistryCredential

> deleteRegistryCredential(id)

Delete a registry credential. Admin only.

# Errors  Returns [&#x60;ApiError::Forbidden&#x60;] for non-admins, [&#x60;ApiError::NotFound&#x60;] when the credential does not exist, or [&#x60;ApiError::Internal&#x60;] when the store fails.

### Example

```ts
import {
  Configuration,
  CredentialsApi,
} from '@zlayer/client';
import type { DeleteRegistryCredentialRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new CredentialsApi(config);

  const body = {
    // string | Registry credential id
    id: id_example,
  } satisfies DeleteRegistryCredentialRequest;

  try {
    const data = await api.deleteRegistryCredential(body);
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
| **id** | `string` | Registry credential id | [Defaults to `undefined`] |

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
| **204** | Registry credential deleted |  -  |
| **403** | Admin role required |  -  |
| **404** | Not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## listGitCredentials

> Array&lt;GitCredentialResponse&gt; listGitCredentials()

List all git credentials (metadata only, no secret values).

# Errors  Returns [&#x60;ApiError::Internal&#x60;] if the credential store fails.

### Example

```ts
import {
  Configuration,
  CredentialsApi,
} from '@zlayer/client';
import type { ListGitCredentialsRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new CredentialsApi(config);

  try {
    const data = await api.listGitCredentials();
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

[**Array&lt;GitCredentialResponse&gt;**](GitCredentialResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | List of git credentials |  -  |
| **401** | Unauthorized |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## listRegistryCredentials

> Array&lt;RegistryCredentialResponse&gt; listRegistryCredentials()

List all registry credentials (metadata only, no passwords).

# Errors  Returns [&#x60;ApiError::Internal&#x60;] if the credential store fails.

### Example

```ts
import {
  Configuration,
  CredentialsApi,
} from '@zlayer/client';
import type { ListRegistryCredentialsRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new CredentialsApi(config);

  try {
    const data = await api.listRegistryCredentials();
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

[**Array&lt;RegistryCredentialResponse&gt;**](RegistryCredentialResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | List of registry credentials |  -  |
| **401** | Unauthorized |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)

