# UsersApi

All URIs are relative to *http://localhost*

| Method | HTTP request | Description |
|------------- | ------------- | -------------|
| [**createUser**](UsersApi.md#createuseroperation) | **POST** /api/v1/users | Create a new user. Admin only. |
| [**deleteUser**](UsersApi.md#deleteuser) | **DELETE** /api/v1/users/{id} | Delete a user. Admin only. Callers cannot delete their own account. |
| [**getUser**](UsersApi.md#getuser) | **GET** /api/v1/users/{id} | Fetch a single user. Admins can read any record; regular users can read only their own. |
| [**listUsers**](UsersApi.md#listusers) | **GET** /api/v1/users | List all users. Admin only. |
| [**setPassword**](UsersApi.md#setpasswordoperation) | **POST** /api/v1/users/{id}/password | Set a user\&#39;s password. Admins may change any user\&#39;s password; regular users may only change their own, and must supply &#x60;current_password&#x60;. |
| [**updateUser**](UsersApi.md#updateuseroperation) | **PATCH** /api/v1/users/{id} | Update a user\&#39;s mutable fields. Admin only. |



## createUser

> UserView createUser(createUserRequest)

Create a new user. Admin only.

# Errors  Returns [&#x60;ApiError::Forbidden&#x60;] when the caller is not an admin, [&#x60;ApiError::BadRequest&#x60;] when the body is incomplete, [&#x60;ApiError::Conflict&#x60;] when the email is already registered, or [&#x60;ApiError::Internal&#x60;] if a backing store fails.

### Example

```ts
import {
  Configuration,
  UsersApi,
} from '@zlayer/client';
import type { CreateUserOperationRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const api = new UsersApi();

  const body = {
    // CreateUserRequest
    createUserRequest: ...,
  } satisfies CreateUserOperationRequest;

  try {
    const data = await api.createUser(body);
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
| **createUserRequest** | [CreateUserRequest](CreateUserRequest.md) |  | |

### Return type

[**UserView**](UserView.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: `application/json`
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **201** | User created |  -  |
| **400** | Invalid request |  -  |
| **403** | Admin role required |  -  |
| **409** | Email already in use |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## deleteUser

> deleteUser(id)

Delete a user. Admin only. Callers cannot delete their own account.

# Errors  Returns [&#x60;ApiError::Forbidden&#x60;] when the caller is not an admin, [&#x60;ApiError::BadRequest&#x60;] when an admin tries to delete themselves, [&#x60;ApiError::NotFound&#x60;] when the user does not exist, or [&#x60;ApiError::Internal&#x60;] if a backing store fails.

### Example

```ts
import {
  Configuration,
  UsersApi,
} from '@zlayer/client';
import type { DeleteUserRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const api = new UsersApi();

  const body = {
    // string | User id
    id: id_example,
  } satisfies DeleteUserRequest;

  try {
    const data = await api.deleteUser(body);
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
| **id** | `string` | User id | [Defaults to `undefined`] |

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
| **204** | Deleted |  -  |
| **403** | Admin role required |  -  |
| **404** | Not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## getUser

> UserView getUser(id)

Fetch a single user. Admins can read any record; regular users can read only their own.

# Errors  Returns [&#x60;ApiError::Forbidden&#x60;] when a non-admin tries to read a different user, [&#x60;ApiError::NotFound&#x60;] when no such user exists, or [&#x60;ApiError::Internal&#x60;] when the store fails.

### Example

```ts
import {
  Configuration,
  UsersApi,
} from '@zlayer/client';
import type { GetUserRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const api = new UsersApi();

  const body = {
    // string | User id
    id: id_example,
  } satisfies GetUserRequest;

  try {
    const data = await api.getUser(body);
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
| **id** | `string` | User id | [Defaults to `undefined`] |

### Return type

[**UserView**](UserView.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | User |  -  |
| **404** | Not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## listUsers

> Array&lt;UserView&gt; listUsers()

List all users. Admin only.

# Errors  Returns [&#x60;ApiError::Forbidden&#x60;] when the caller is not an admin, or [&#x60;ApiError::Internal&#x60;] if the user store fails.

### Example

```ts
import {
  Configuration,
  UsersApi,
} from '@zlayer/client';
import type { ListUsersRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const api = new UsersApi();

  try {
    const data = await api.listUsers();
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

[**Array&lt;UserView&gt;**](UserView.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | List users |  -  |
| **403** | Admin role required |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## setPassword

> setPassword(id, setPasswordRequest)

Set a user\&#39;s password. Admins may change any user\&#39;s password; regular users may only change their own, and must supply &#x60;current_password&#x60;.

# Errors  Returns [&#x60;ApiError::BadRequest&#x60;] for missing fields, [&#x60;ApiError::Unauthorized&#x60;] for an incorrect current password, [&#x60;ApiError::Forbidden&#x60;] when the caller is not allowed to change this user\&#39;s password, [&#x60;ApiError::NotFound&#x60;] when the user does not exist, or [&#x60;ApiError::Internal&#x60;] if a backing store fails.

### Example

```ts
import {
  Configuration,
  UsersApi,
} from '@zlayer/client';
import type { SetPasswordOperationRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const api = new UsersApi();

  const body = {
    // string | User id
    id: id_example,
    // SetPasswordRequest
    setPasswordRequest: ...,
  } satisfies SetPasswordOperationRequest;

  try {
    const data = await api.setPassword(body);
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
| **id** | `string` | User id | [Defaults to `undefined`] |
| **setPasswordRequest** | [SetPasswordRequest](SetPasswordRequest.md) |  | |

### Return type

`void` (Empty response body)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: `application/json`
- **Accept**: Not defined


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **204** | Password updated |  -  |
| **400** | Invalid request |  -  |
| **401** | Current password incorrect (self-service path) |  -  |
| **403** | Not permitted |  -  |
| **404** | Not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## updateUser

> UserView updateUser(id, updateUserRequest)

Update a user\&#39;s mutable fields. Admin only.

# Errors  Returns [&#x60;ApiError::Forbidden&#x60;] when the caller is not an admin, [&#x60;ApiError::NotFound&#x60;] when the user does not exist, or [&#x60;ApiError::Internal&#x60;] if the user store fails.

### Example

```ts
import {
  Configuration,
  UsersApi,
} from '@zlayer/client';
import type { UpdateUserOperationRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const api = new UsersApi();

  const body = {
    // string | User id
    id: id_example,
    // UpdateUserRequest
    updateUserRequest: ...,
  } satisfies UpdateUserOperationRequest;

  try {
    const data = await api.updateUser(body);
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
| **id** | `string` | User id | [Defaults to `undefined`] |
| **updateUserRequest** | [UpdateUserRequest](UpdateUserRequest.md) |  | |

### Return type

[**UserView**](UserView.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: `application/json`
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Updated user |  -  |
| **403** | Admin role required |  -  |
| **404** | Not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)

