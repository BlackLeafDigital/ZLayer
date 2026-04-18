# GroupsApi

All URIs are relative to *http://localhost*

| Method | HTTP request | Description |
|------------- | ------------- | -------------|
| [**addMember**](GroupsApi.md#addmemberoperation) | **POST** /api/v1/groups/{id}/members | Add a member to a group. Admin only. |
| [**createGroup**](GroupsApi.md#creategroupoperation) | **POST** /api/v1/groups | Create a new group. Admin only. |
| [**deleteGroup**](GroupsApi.md#deletegroup) | **DELETE** /api/v1/groups/{id} | Delete a group. Admin only. |
| [**getGroup**](GroupsApi.md#getgroup) | **GET** /api/v1/groups/{id} | Fetch a single group by id. |
| [**listGroups**](GroupsApi.md#listgroups) | **GET** /api/v1/groups | List all groups. |
| [**removeMember**](GroupsApi.md#removemember) | **DELETE** /api/v1/groups/{id}/members/{user_id} | Remove a member from a group. Admin only. |
| [**updateGroup**](GroupsApi.md#updategroupoperation) | **PATCH** /api/v1/groups/{id} | Update a group\&#39;s name and/or description. Admin only. |



## addMember

> addMember(id, addMemberRequest)

Add a member to a group. Admin only.

# Errors  Returns [&#x60;ApiError::Forbidden&#x60;] when the caller is not an admin, [&#x60;ApiError::BadRequest&#x60;] when &#x60;user_id&#x60; is empty, [&#x60;ApiError::NotFound&#x60;] when the group does not exist, or [&#x60;ApiError::Internal&#x60;] if the store fails.

### Example

```ts
import {
  Configuration,
  GroupsApi,
} from '@zlayer/client';
import type { AddMemberOperationRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const api = new GroupsApi();

  const body = {
    // string | Group id
    id: id_example,
    // AddMemberRequest
    addMemberRequest: ...,
  } satisfies AddMemberOperationRequest;

  try {
    const data = await api.addMember(body);
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
| **id** | `string` | Group id | [Defaults to `undefined`] |
| **addMemberRequest** | [AddMemberRequest](AddMemberRequest.md) |  | |

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
| **204** | Member added |  -  |
| **400** | Invalid request |  -  |
| **403** | Admin role required |  -  |
| **404** | Group not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## createGroup

> StoredUserGroup createGroup(createGroupRequest)

Create a new group. Admin only.

# Errors  Returns [&#x60;ApiError::Forbidden&#x60;] when the caller is not an admin, [&#x60;ApiError::BadRequest&#x60;] when the name is empty, or [&#x60;ApiError::Internal&#x60;] if the store fails.

### Example

```ts
import {
  Configuration,
  GroupsApi,
} from '@zlayer/client';
import type { CreateGroupOperationRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const api = new GroupsApi();

  const body = {
    // CreateGroupRequest
    createGroupRequest: ...,
  } satisfies CreateGroupOperationRequest;

  try {
    const data = await api.createGroup(body);
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
| **createGroupRequest** | [CreateGroupRequest](CreateGroupRequest.md) |  | |

### Return type

[**StoredUserGroup**](StoredUserGroup.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: `application/json`
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **201** | Group created |  -  |
| **400** | Invalid request |  -  |
| **403** | Admin role required |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## deleteGroup

> deleteGroup(id)

Delete a group. Admin only.

# Errors  Returns [&#x60;ApiError::Forbidden&#x60;] when the caller is not an admin, [&#x60;ApiError::NotFound&#x60;] when the group does not exist, or [&#x60;ApiError::Internal&#x60;] if the store fails.

### Example

```ts
import {
  Configuration,
  GroupsApi,
} from '@zlayer/client';
import type { DeleteGroupRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const api = new GroupsApi();

  const body = {
    // string | Group id
    id: id_example,
  } satisfies DeleteGroupRequest;

  try {
    const data = await api.deleteGroup(body);
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
| **id** | `string` | Group id | [Defaults to `undefined`] |

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


## getGroup

> StoredUserGroup getGroup(id)

Fetch a single group by id.

# Errors  Returns [&#x60;ApiError::NotFound&#x60;] when no such group exists, or [&#x60;ApiError::Internal&#x60;] when the store fails.

### Example

```ts
import {
  Configuration,
  GroupsApi,
} from '@zlayer/client';
import type { GetGroupRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const api = new GroupsApi();

  const body = {
    // string | Group id
    id: id_example,
  } satisfies GetGroupRequest;

  try {
    const data = await api.getGroup(body);
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
| **id** | `string` | Group id | [Defaults to `undefined`] |

### Return type

[**StoredUserGroup**](StoredUserGroup.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Group |  -  |
| **404** | Not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## listGroups

> Array&lt;StoredUserGroup&gt; listGroups()

List all groups.

# Errors  Returns [&#x60;ApiError::Internal&#x60;] if the group store fails.

### Example

```ts
import {
  Configuration,
  GroupsApi,
} from '@zlayer/client';
import type { ListGroupsRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const api = new GroupsApi();

  try {
    const data = await api.listGroups();
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

[**Array&lt;StoredUserGroup&gt;**](StoredUserGroup.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | List groups |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## removeMember

> removeMember(id, userId)

Remove a member from a group. Admin only.

# Errors  Returns [&#x60;ApiError::Forbidden&#x60;] when the caller is not an admin, [&#x60;ApiError::NotFound&#x60;] when the group or membership does not exist, or [&#x60;ApiError::Internal&#x60;] if the store fails.

### Example

```ts
import {
  Configuration,
  GroupsApi,
} from '@zlayer/client';
import type { RemoveMemberRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const api = new GroupsApi();

  const body = {
    // string | Group id
    id: id_example,
    // string | User id to remove
    userId: userId_example,
  } satisfies RemoveMemberRequest;

  try {
    const data = await api.removeMember(body);
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
| **id** | `string` | Group id | [Defaults to `undefined`] |
| **userId** | `string` | User id to remove | [Defaults to `undefined`] |

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
| **204** | Member removed |  -  |
| **403** | Admin role required |  -  |
| **404** | Group or membership not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## updateGroup

> StoredUserGroup updateGroup(id, updateGroupRequest)

Update a group\&#39;s name and/or description. Admin only.

# Errors  Returns [&#x60;ApiError::Forbidden&#x60;] when the caller is not an admin, [&#x60;ApiError::NotFound&#x60;] when the group does not exist, or [&#x60;ApiError::Internal&#x60;] if the store fails.

### Example

```ts
import {
  Configuration,
  GroupsApi,
} from '@zlayer/client';
import type { UpdateGroupOperationRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const api = new GroupsApi();

  const body = {
    // string | Group id
    id: id_example,
    // UpdateGroupRequest
    updateGroupRequest: ...,
  } satisfies UpdateGroupOperationRequest;

  try {
    const data = await api.updateGroup(body);
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
| **id** | `string` | Group id | [Defaults to `undefined`] |
| **updateGroupRequest** | [UpdateGroupRequest](UpdateGroupRequest.md) |  | |

### Return type

[**StoredUserGroup**](StoredUserGroup.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: `application/json`
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Updated group |  -  |
| **403** | Admin role required |  -  |
| **404** | Not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)

