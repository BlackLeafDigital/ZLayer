# SecretsApi

All URIs are relative to *http://localhost*

| Method | HTTP request | Description |
|------------- | ------------- | -------------|
| [**bulkImportSecrets**](SecretsApi.md#bulkimportsecrets) | **POST** /api/v1/secrets/bulk-import | Bulk-import secrets from a dotenv-style payload (&#x60;KEY&#x3D;value\\n…&#x60;). |
| [**createSecret**](SecretsApi.md#createsecretoperation) | **POST** /api/v1/secrets | Create or update a secret. |
| [**deleteSecret**](SecretsApi.md#deletesecret) | **DELETE** /api/v1/secrets/{name} | Delete a secret. |
| [**getSecretMetadata**](SecretsApi.md#getsecretmetadata) | **GET** /api/v1/secrets/{name} | Get metadata for a specific secret. With &#x60;?reveal&#x3D;true&#x60; (admin only), the response also includes the plaintext &#x60;value&#x60;. |
| [**listSecrets**](SecretsApi.md#listsecrets) | **GET** /api/v1/secrets | List secrets in a scope. |



## bulkImportSecrets

> BulkImportResponse bulkImportSecrets(environment, body)

Bulk-import secrets from a dotenv-style payload (&#x60;KEY&#x3D;value\\n…&#x60;).

Each non-empty, non-comment line is parsed into a (name, value) pair and written to the env\&#39;s scope. Lines that fail to parse are returned in &#x60;errors&#x60; and do not abort the import. Each successful write is counted as either &#x60;created&#x60; or &#x60;updated&#x60;.  # Errors  Returns [&#x60;ApiError::Forbidden&#x60;] for non-admins, [&#x60;ApiError::NotFound&#x60;] when the environment id is unknown, or [&#x60;ApiError::Internal&#x60;] when the secrets store fails.

### Example

```ts
import {
  Configuration,
  SecretsApi,
} from '@zlayer/client';
import type { BulkImportSecretsRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new SecretsApi(config);

  const body = {
    // string | Environment id to import into
    environment: environment_example,
    // string
    body: body_example,
  } satisfies BulkImportSecretsRequest;

  try {
    const data = await api.bulkImportSecrets(body);
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
| **environment** | `string` | Environment id to import into | [Defaults to `undefined`] |
| **body** | `string` |  | |

### Return type

[**BulkImportResponse**](BulkImportResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: `text/plain`
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Import summary |  -  |
| **400** | Invalid request |  -  |
| **403** | Admin role required |  -  |
| **404** | Environment id unknown |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## createSecret

> SecretMetadataResponse createSecret(createSecretRequest, environment, scope)

Create or update a secret.

Stores a new secret or updates an existing one. The secret value is encrypted at rest and the version number is incremented on updates.  Scope resolution: see [&#x60;resolve_scope&#x60;].  # Errors  Returns an error if validation fails, storage operations fail, or the caller lacks permission.

### Example

```ts
import {
  Configuration,
  SecretsApi,
} from '@zlayer/client';
import type { CreateSecretOperationRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new SecretsApi(config);

  const body = {
    // CreateSecretRequest
    createSecretRequest: ...,
    // string | Environment id (mutually exclusive with body \'scope\') (optional)
    environment: environment_example,
    // string | Explicit scope (legacy) (optional)
    scope: scope_example,
  } satisfies CreateSecretOperationRequest;

  try {
    const data = await api.createSecret(body);
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
| **createSecretRequest** | [CreateSecretRequest](CreateSecretRequest.md) |  | |
| **environment** | `string` | Environment id (mutually exclusive with body \&#39;scope\&#39;) | [Optional] [Defaults to `undefined`] |
| **scope** | `string` | Explicit scope (legacy) | [Optional] [Defaults to `undefined`] |

### Return type

[**SecretMetadataResponse**](SecretMetadataResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: `application/json`
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Secret updated |  -  |
| **201** | Secret created |  -  |
| **400** | Invalid request |  -  |
| **401** | Unauthorized |  -  |
| **403** | Forbidden |  -  |
| **404** | Environment id unknown |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## deleteSecret

> deleteSecret(name, environment, scope)

Delete a secret.

Scope resolution: see [&#x60;resolve_scope_get&#x60;] (the same query type is reused minus &#x60;reveal&#x60;).  # Errors  Returns an error if the secret is not found, storage access fails, or the caller lacks permission.

### Example

```ts
import {
  Configuration,
  SecretsApi,
} from '@zlayer/client';
import type { DeleteSecretRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new SecretsApi(config);

  const body = {
    // string | Secret name
    name: name_example,
    // string | Environment id (optional)
    environment: environment_example,
    // string | Explicit scope (optional)
    scope: scope_example,
  } satisfies DeleteSecretRequest;

  try {
    const data = await api.deleteSecret(body);
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
| **name** | `string` | Secret name | [Defaults to `undefined`] |
| **environment** | `string` | Environment id | [Optional] [Defaults to `undefined`] |
| **scope** | `string` | Explicit scope | [Optional] [Defaults to `undefined`] |

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
| **204** | Secret deleted |  -  |
| **401** | Unauthorized |  -  |
| **403** | Forbidden |  -  |
| **404** | Secret not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## getSecretMetadata

> SecretMetadataResponse getSecretMetadata(name, environment, scope, reveal)

Get metadata for a specific secret. With &#x60;?reveal&#x3D;true&#x60; (admin only), the response also includes the plaintext &#x60;value&#x60;.

Scope resolution: see [&#x60;resolve_scope_get&#x60;].  # Errors  Returns an error if the secret is not found, the caller is unauthorised to reveal, or storage access fails.

### Example

```ts
import {
  Configuration,
  SecretsApi,
} from '@zlayer/client';
import type { GetSecretMetadataRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new SecretsApi(config);

  const body = {
    // string | Secret name
    name: name_example,
    // string | Environment id (optional)
    environment: environment_example,
    // string | Explicit scope (optional)
    scope: scope_example,
    // boolean | Include plaintext value (admin only) (optional)
    reveal: true,
  } satisfies GetSecretMetadataRequest;

  try {
    const data = await api.getSecretMetadata(body);
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
| **name** | `string` | Secret name | [Defaults to `undefined`] |
| **environment** | `string` | Environment id | [Optional] [Defaults to `undefined`] |
| **scope** | `string` | Explicit scope | [Optional] [Defaults to `undefined`] |
| **reveal** | `boolean` | Include plaintext value (admin only) | [Optional] [Defaults to `undefined`] |

### Return type

[**SecretMetadataResponse**](SecretMetadataResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Secret metadata |  -  |
| **401** | Unauthorized |  -  |
| **403** | Reveal requires admin |  -  |
| **404** | Secret not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## listSecrets

> Array&lt;SecretMetadataResponse&gt; listSecrets(environment, scope)

List secrets in a scope.

Scope resolution: see [&#x60;resolve_scope&#x60;].  # Errors  Returns an error if storage access fails.

### Example

```ts
import {
  Configuration,
  SecretsApi,
} from '@zlayer/client';
import type { ListSecretsRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new SecretsApi(config);

  const body = {
    // string | Environment id (optional)
    environment: environment_example,
    // string | Explicit scope (optional)
    scope: scope_example,
  } satisfies ListSecretsRequest;

  try {
    const data = await api.listSecrets(body);
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
| **environment** | `string` | Environment id | [Optional] [Defaults to `undefined`] |
| **scope** | `string` | Explicit scope | [Optional] [Defaults to `undefined`] |

### Return type

[**Array&lt;SecretMetadataResponse&gt;**](SecretMetadataResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | List of secret metadata |  -  |
| **401** | Unauthorized |  -  |
| **404** | Environment id unknown |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)

