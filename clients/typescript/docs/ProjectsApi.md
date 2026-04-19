# ProjectsApi

All URIs are relative to *http://localhost*

| Method | HTTP request | Description |
|------------- | ------------- | -------------|
| [**createProject**](ProjectsApi.md#createprojectoperation) | **POST** /api/v1/projects | Create a new project. Admin only. |
| [**deleteProject**](ProjectsApi.md#deleteproject) | **DELETE** /api/v1/projects/{id} | Delete a project. Admin only. Cascade-removes deployment links. |
| [**getProject**](ProjectsApi.md#getproject) | **GET** /api/v1/projects/{id} | Fetch a single project by id. |
| [**linkProjectDeployment**](ProjectsApi.md#linkprojectdeployment) | **POST** /api/v1/projects/{id}/deployments | Link a deployment to a project. |
| [**listProjectDeployments**](ProjectsApi.md#listprojectdeployments) | **GET** /api/v1/projects/{id}/deployments | List deployment names linked to a project. |
| [**listProjects**](ProjectsApi.md#listprojects) | **GET** /api/v1/projects | List all projects. |
| [**pullProject**](ProjectsApi.md#pullproject) | **POST** /api/v1/projects/{id}/pull | Clone the project\&#39;s git repository (or fast-forward pull if the working copy already exists) into &#x60;{clone_root}/{project_id}&#x60; and return the resulting HEAD SHA. |
| [**unlinkProjectDeployment**](ProjectsApi.md#unlinkprojectdeployment) | **DELETE** /api/v1/projects/{id}/deployments/{name} | Unlink a deployment from a project. |
| [**updateProject**](ProjectsApi.md#updateprojectoperation) | **PATCH** /api/v1/projects/{id} | Update a project. Admin only. |



## createProject

> StoredProject createProject(createProjectRequest)

Create a new project. Admin only.

# Errors  Returns [&#x60;ApiError::Forbidden&#x60;] when the caller is not an admin, [&#x60;ApiError::BadRequest&#x60;] for an empty name, [&#x60;ApiError::Conflict&#x60;] when the name is already used, or [&#x60;ApiError::Internal&#x60;] when the project store fails.

### Example

```ts
import {
  Configuration,
  ProjectsApi,
} from '@zlayer/client';
import type { CreateProjectOperationRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new ProjectsApi(config);

  const body = {
    // CreateProjectRequest
    createProjectRequest: ...,
  } satisfies CreateProjectOperationRequest;

  try {
    const data = await api.createProject(body);
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
| **createProjectRequest** | [CreateProjectRequest](CreateProjectRequest.md) |  | |

### Return type

[**StoredProject**](StoredProject.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: `application/json`
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **201** | Project created |  -  |
| **400** | Invalid request |  -  |
| **403** | Admin role required |  -  |
| **409** | Name already used |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## deleteProject

> deleteProject(id)

Delete a project. Admin only. Cascade-removes deployment links.

# Errors  Returns [&#x60;ApiError::Forbidden&#x60;] for non-admins, [&#x60;ApiError::NotFound&#x60;] when the project does not exist, or [&#x60;ApiError::Internal&#x60;] when the store fails.

### Example

```ts
import {
  Configuration,
  ProjectsApi,
} from '@zlayer/client';
import type { DeleteProjectRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new ProjectsApi(config);

  const body = {
    // string | Project id
    id: id_example,
  } satisfies DeleteProjectRequest;

  try {
    const data = await api.deleteProject(body);
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
| **id** | `string` | Project id | [Defaults to `undefined`] |

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
| **204** | Project deleted |  -  |
| **403** | Admin role required |  -  |
| **404** | Not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## getProject

> StoredProject getProject(id)

Fetch a single project by id.

# Errors  Returns [&#x60;ApiError::NotFound&#x60;] if no project with the given id exists, or [&#x60;ApiError::Internal&#x60;] when the store fails.

### Example

```ts
import {
  Configuration,
  ProjectsApi,
} from '@zlayer/client';
import type { GetProjectRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new ProjectsApi(config);

  const body = {
    // string | Project id
    id: id_example,
  } satisfies GetProjectRequest;

  try {
    const data = await api.getProject(body);
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
| **id** | `string` | Project id | [Defaults to `undefined`] |

### Return type

[**StoredProject**](StoredProject.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Project |  -  |
| **404** | Not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## linkProjectDeployment

> linkProjectDeployment(id, linkDeploymentRequest)

Link a deployment to a project.

# Errors  Returns [&#x60;ApiError::NotFound&#x60;] when the project does not exist, [&#x60;ApiError::BadRequest&#x60;] for an empty deployment name, or [&#x60;ApiError::Internal&#x60;] when the store fails.

### Example

```ts
import {
  Configuration,
  ProjectsApi,
} from '@zlayer/client';
import type { LinkProjectDeploymentRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new ProjectsApi(config);

  const body = {
    // string | Project id
    id: id_example,
    // LinkDeploymentRequest
    linkDeploymentRequest: ...,
  } satisfies LinkProjectDeploymentRequest;

  try {
    const data = await api.linkProjectDeployment(body);
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
| **id** | `string` | Project id | [Defaults to `undefined`] |
| **linkDeploymentRequest** | [LinkDeploymentRequest](LinkDeploymentRequest.md) |  | |

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
| **201** | Deployment linked |  -  |
| **400** | Invalid request |  -  |
| **404** | Not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## listProjectDeployments

> Array&lt;string&gt; listProjectDeployments(id)

List deployment names linked to a project.

# Errors  Returns [&#x60;ApiError::NotFound&#x60;] when the project does not exist, or [&#x60;ApiError::Internal&#x60;] when the store fails.

### Example

```ts
import {
  Configuration,
  ProjectsApi,
} from '@zlayer/client';
import type { ListProjectDeploymentsRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new ProjectsApi(config);

  const body = {
    // string | Project id
    id: id_example,
  } satisfies ListProjectDeploymentsRequest;

  try {
    const data = await api.listProjectDeployments(body);
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
| **id** | `string` | Project id | [Defaults to `undefined`] |

### Return type

**Array<string>**

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Linked deployment names |  -  |
| **404** | Not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## listProjects

> Array&lt;StoredProject&gt; listProjects()

List all projects.

Any authenticated user can list projects.  # Errors  Returns [&#x60;ApiError::Internal&#x60;] if the project store fails.

### Example

```ts
import {
  Configuration,
  ProjectsApi,
} from '@zlayer/client';
import type { ListProjectsRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new ProjectsApi(config);

  try {
    const data = await api.listProjects();
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

[**Array&lt;StoredProject&gt;**](StoredProject.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | List of projects |  -  |
| **401** | Unauthorized |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## pullProject

> ProjectPullResponse pullProject(id)

Clone the project\&#39;s git repository (or fast-forward pull if the working copy already exists) into &#x60;{clone_root}/{project_id}&#x60; and return the resulting HEAD SHA.

Authentication is resolved from &#x60;git_credential_id&#x60;: when set, the matching [&#x60;GitCredential&#x60;](zlayer_secrets::GitCredential) is looked up in the [&#x60;GitCredentialStore&#x60;](zlayer_secrets::GitCredentialStore) and its kind determines whether we build a PAT or SSH auth config. When the project has no credential id, or the state has no git credential store attached, we fall back to anonymous auth.  Admin only.  # Errors  Returns [&#x60;ApiError::Forbidden&#x60;] for non-admins, [&#x60;ApiError::NotFound&#x60;] when the project does not exist, [&#x60;ApiError::BadRequest&#x60;] when the project has no &#x60;git_url&#x60; or its &#x60;git_credential_id&#x60; cannot be resolved, or [&#x60;ApiError::Internal&#x60;] on any underlying git / credential store failure.

### Example

```ts
import {
  Configuration,
  ProjectsApi,
} from '@zlayer/client';
import type { PullProjectRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new ProjectsApi(config);

  const body = {
    // string | Project id
    id: id_example,
  } satisfies PullProjectRequest;

  try {
    const data = await api.pullProject(body);
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
| **id** | `string` | Project id | [Defaults to `undefined`] |

### Return type

[**ProjectPullResponse**](ProjectPullResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Pull succeeded |  -  |
| **400** | Project has no git URL configured |  -  |
| **403** | Admin role required |  -  |
| **404** | Project not found |  -  |
| **500** | Git operation failed |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## unlinkProjectDeployment

> unlinkProjectDeployment(id, name)

Unlink a deployment from a project.

# Errors  Returns [&#x60;ApiError::NotFound&#x60;] when the project or link does not exist, or [&#x60;ApiError::Internal&#x60;] when the store fails.

### Example

```ts
import {
  Configuration,
  ProjectsApi,
} from '@zlayer/client';
import type { UnlinkProjectDeploymentRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new ProjectsApi(config);

  const body = {
    // string | Project id
    id: id_example,
    // string | Deployment name
    name: name_example,
  } satisfies UnlinkProjectDeploymentRequest;

  try {
    const data = await api.unlinkProjectDeployment(body);
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
| **id** | `string` | Project id | [Defaults to `undefined`] |
| **name** | `string` | Deployment name | [Defaults to `undefined`] |

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
| **204** | Deployment unlinked |  -  |
| **404** | Not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## updateProject

> StoredProject updateProject(id, updateProjectRequest)

Update a project. Admin only.

# Errors  Returns [&#x60;ApiError::Forbidden&#x60;] for non-admins, [&#x60;ApiError::NotFound&#x60;] when the project does not exist, [&#x60;ApiError::BadRequest&#x60;] for an empty replacement name, [&#x60;ApiError::Conflict&#x60;] when the new name collides, or [&#x60;ApiError::Internal&#x60;] when the store fails.

### Example

```ts
import {
  Configuration,
  ProjectsApi,
} from '@zlayer/client';
import type { UpdateProjectOperationRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new ProjectsApi(config);

  const body = {
    // string | Project id
    id: id_example,
    // UpdateProjectRequest
    updateProjectRequest: ...,
  } satisfies UpdateProjectOperationRequest;

  try {
    const data = await api.updateProject(body);
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
| **id** | `string` | Project id | [Defaults to `undefined`] |
| **updateProjectRequest** | [UpdateProjectRequest](UpdateProjectRequest.md) |  | |

### Return type

[**StoredProject**](StoredProject.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: `application/json`
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Updated project |  -  |
| **400** | Invalid request |  -  |
| **403** | Admin role required |  -  |
| **404** | Not found |  -  |
| **409** | Name collides with another project |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)

