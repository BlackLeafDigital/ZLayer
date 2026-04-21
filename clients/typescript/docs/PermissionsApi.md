# PermissionsApi

All URIs are relative to *http://localhost*

| Method | HTTP request | Description |
|------------- | ------------- | -------------|
| [**grantPermission**](PermissionsApi.md#grantpermissionoperation) | **POST** /api/v1/permissions | Grant a permission. Admin only. |
| [**listPermissions**](PermissionsApi.md#listpermissions) | **GET** /api/v1/permissions | List permissions for a subject (user or group). |
| [**listPermissionsByResource**](PermissionsApi.md#listpermissionsbyresource) | **GET** /api/v1/permissions/by-resource | List permissions granted on a specific resource. |
| [**revokePermission**](PermissionsApi.md#revokepermission) | **DELETE** /api/v1/permissions/{id} | Revoke a permission by id. Admin only. |



## grantPermission

> StoredPermission grantPermission(grantPermissionRequest)

Grant a permission. Admin only.

# Errors  Returns [&#x60;ApiError::Forbidden&#x60;] when the caller is not an admin, [&#x60;ApiError::BadRequest&#x60;] when required fields are missing, or [&#x60;ApiError::Internal&#x60;] if the store fails.

### Example

```ts
import {
  Configuration,
  PermissionsApi,
} from '@zlayer/api-client';
import type { GrantPermissionOperationRequest } from '@zlayer/api-client';

async function example() {
  console.log("🚀 Testing @zlayer/api-client SDK...");
  const api = new PermissionsApi();

  const body = {
    // GrantPermissionRequest
    grantPermissionRequest: ...,
  } satisfies GrantPermissionOperationRequest;

  try {
    const data = await api.grantPermission(body);
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
| **grantPermissionRequest** | [GrantPermissionRequest](GrantPermissionRequest.md) |  | |

### Return type

[**StoredPermission**](StoredPermission.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: `application/json`
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **201** | Permission granted |  -  |
| **400** | Invalid request |  -  |
| **403** | Admin role required |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## listPermissions

> Array&lt;StoredPermission&gt; listPermissions(user, group)

List permissions for a subject (user or group).

Exactly one of &#x60;user&#x60; or &#x60;group&#x60; must be provided.  # Errors  Returns [&#x60;ApiError::BadRequest&#x60;] when neither or both query parameters are provided, or [&#x60;ApiError::Internal&#x60;] if the store fails.

### Example

```ts
import {
  Configuration,
  PermissionsApi,
} from '@zlayer/api-client';
import type { ListPermissionsRequest } from '@zlayer/api-client';

async function example() {
  console.log("🚀 Testing @zlayer/api-client SDK...");
  const api = new PermissionsApi();

  const body = {
    // string | Filter by user id. (optional)
    user: user_example,
    // string | Filter by group id. (optional)
    group: group_example,
  } satisfies ListPermissionsRequest;

  try {
    const data = await api.listPermissions(body);
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
| **user** | `string` | Filter by user id. | [Optional] [Defaults to `undefined`] |
| **group** | `string` | Filter by group id. | [Optional] [Defaults to `undefined`] |

### Return type

[**Array&lt;StoredPermission&gt;**](StoredPermission.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | List permissions |  -  |
| **400** | Must provide exactly one of user or group |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## listPermissionsByResource

> Array&lt;StoredPermission&gt; listPermissionsByResource(kind, id)

List permissions granted on a specific resource.

When &#x60;id&#x60; is supplied, returns exact-resource grants; when omitted, returns wildcard grants for the given &#x60;kind&#x60;.  # Errors  Returns [&#x60;ApiError::Internal&#x60;] on store failure.

### Example

```ts
import {
  Configuration,
  PermissionsApi,
} from '@zlayer/api-client';
import type { ListPermissionsByResourceRequest } from '@zlayer/api-client';

async function example() {
  console.log("🚀 Testing @zlayer/api-client SDK...");
  const api = new PermissionsApi();

  const body = {
    // string | Resource kind (e.g. `\"environment\"`).
    kind: kind_example,
    // string | Specific resource id. Omit for wildcard grants only. (optional)
    id: id_example,
  } satisfies ListPermissionsByResourceRequest;

  try {
    const data = await api.listPermissionsByResource(body);
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
| **kind** | `string` | Resource kind (e.g. &#x60;\&quot;environment\&quot;&#x60;). | [Defaults to `undefined`] |
| **id** | `string` | Specific resource id. Omit for wildcard grants only. | [Optional] [Defaults to `undefined`] |

### Return type

[**Array&lt;StoredPermission&gt;**](StoredPermission.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Grants on the resource |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## revokePermission

> revokePermission(id)

Revoke a permission by id. Admin only.

# Errors  Returns [&#x60;ApiError::Forbidden&#x60;] when the caller is not an admin, [&#x60;ApiError::NotFound&#x60;] when the permission does not exist, or [&#x60;ApiError::Internal&#x60;] if the store fails.

### Example

```ts
import {
  Configuration,
  PermissionsApi,
} from '@zlayer/api-client';
import type { RevokePermissionRequest } from '@zlayer/api-client';

async function example() {
  console.log("🚀 Testing @zlayer/api-client SDK...");
  const api = new PermissionsApi();

  const body = {
    // string | Permission id
    id: id_example,
  } satisfies RevokePermissionRequest;

  try {
    const data = await api.revokePermission(body);
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
| **id** | `string` | Permission id | [Defaults to `undefined`] |

### Return type

`void` (Empty response body)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: Not defined


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **204** | Permission revoked |  -  |
| **403** | Admin role required |  -  |
| **404** | Permission not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)

