# WorkflowsApi

All URIs are relative to *http://localhost*

| Method | HTTP request | Description |
|------------- | ------------- | -------------|
| [**createWorkflow**](WorkflowsApi.md#createworkflowoperation) | **POST** /api/v1/workflows | Create a new workflow. Admin only. |
| [**deleteWorkflow**](WorkflowsApi.md#deleteworkflow) | **DELETE** /api/v1/workflows/{id} | Delete a workflow. Admin only. |
| [**getWorkflow**](WorkflowsApi.md#getworkflow) | **GET** /api/v1/workflows/{id} | Fetch a single workflow by id. |
| [**listWorkflowRuns**](WorkflowsApi.md#listworkflowruns) | **GET** /api/v1/workflows/{id}/runs | List past runs for a workflow, most recent first. |
| [**listWorkflows**](WorkflowsApi.md#listworkflows) | **GET** /api/v1/workflows | List workflows. |
| [**runWorkflow**](WorkflowsApi.md#runworkflow) | **POST** /api/v1/workflows/{id}/run | Execute a workflow synchronously. Admin only. |



## createWorkflow

> StoredWorkflow createWorkflow(createWorkflowRequest)

Create a new workflow. Admin only.

# Errors  Returns [&#x60;ApiError::Forbidden&#x60;] when the caller is not an admin, [&#x60;ApiError::BadRequest&#x60;] for an empty name or empty steps list, or [&#x60;ApiError::Internal&#x60;] when the workflow store fails.

### Example

```ts
import {
  Configuration,
  WorkflowsApi,
} from '@zlayer/client';
import type { CreateWorkflowOperationRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new WorkflowsApi(config);

  const body = {
    // CreateWorkflowRequest
    createWorkflowRequest: ...,
  } satisfies CreateWorkflowOperationRequest;

  try {
    const data = await api.createWorkflow(body);
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
| **createWorkflowRequest** | [CreateWorkflowRequest](CreateWorkflowRequest.md) |  | |

### Return type

[**StoredWorkflow**](StoredWorkflow.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: `application/json`
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **201** | Workflow created |  -  |
| **400** | Invalid request |  -  |
| **403** | Admin role required |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## deleteWorkflow

> deleteWorkflow(id)

Delete a workflow. Admin only.

# Errors  Returns [&#x60;ApiError::Forbidden&#x60;] for non-admins, [&#x60;ApiError::NotFound&#x60;] when the workflow does not exist, or [&#x60;ApiError::Internal&#x60;] when the store fails.

### Example

```ts
import {
  Configuration,
  WorkflowsApi,
} from '@zlayer/client';
import type { DeleteWorkflowRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new WorkflowsApi(config);

  const body = {
    // string | Workflow id
    id: id_example,
  } satisfies DeleteWorkflowRequest;

  try {
    const data = await api.deleteWorkflow(body);
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
| **id** | `string` | Workflow id | [Defaults to `undefined`] |

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
| **204** | Workflow deleted |  -  |
| **403** | Admin role required |  -  |
| **404** | Not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## getWorkflow

> StoredWorkflow getWorkflow(id)

Fetch a single workflow by id.

# Errors  Returns [&#x60;ApiError::NotFound&#x60;] if no workflow with the given id exists, or [&#x60;ApiError::Internal&#x60;] when the store fails.

### Example

```ts
import {
  Configuration,
  WorkflowsApi,
} from '@zlayer/client';
import type { GetWorkflowRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new WorkflowsApi(config);

  const body = {
    // string | Workflow id
    id: id_example,
  } satisfies GetWorkflowRequest;

  try {
    const data = await api.getWorkflow(body);
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
| **id** | `string` | Workflow id | [Defaults to `undefined`] |

### Return type

[**StoredWorkflow**](StoredWorkflow.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Workflow |  -  |
| **404** | Not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## listWorkflowRuns

> Array&lt;WorkflowRun&gt; listWorkflowRuns(id)

List past runs for a workflow, most recent first.

# Errors  Returns [&#x60;ApiError::NotFound&#x60;] if the workflow does not exist, or [&#x60;ApiError::Internal&#x60;] when the store fails.

### Example

```ts
import {
  Configuration,
  WorkflowsApi,
} from '@zlayer/client';
import type { ListWorkflowRunsRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new WorkflowsApi(config);

  const body = {
    // string | Workflow id
    id: id_example,
  } satisfies ListWorkflowRunsRequest;

  try {
    const data = await api.listWorkflowRuns(body);
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
| **id** | `string` | Workflow id | [Defaults to `undefined`] |

### Return type

[**Array&lt;WorkflowRun&gt;**](WorkflowRun.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | List of workflow runs |  -  |
| **404** | Workflow not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## listWorkflows

> Array&lt;StoredWorkflow&gt; listWorkflows()

List workflows.

# Errors  Returns [&#x60;ApiError::Internal&#x60;] if the workflow store fails.

### Example

```ts
import {
  Configuration,
  WorkflowsApi,
} from '@zlayer/client';
import type { ListWorkflowsRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new WorkflowsApi(config);

  try {
    const data = await api.listWorkflows();
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

[**Array&lt;StoredWorkflow&gt;**](StoredWorkflow.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | List of workflows |  -  |
| **401** | Unauthorized |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## runWorkflow

> WorkflowRun runWorkflow(id)

Execute a workflow synchronously. Admin only.

Iterates steps sequentially: - &#x60;RunTask&#x60; — looks up the task and executes its &#x60;body&#x60; via &#x60;sh -c&#x60;. - &#x60;BuildProject&#x60; — clones / fast-forwards the project\&#39;s git repo, then   registers and awaits a real buildah build. - &#x60;DeployProject&#x60; — reads the project\&#39;s &#x60;deploy_spec_path&#x60; from its   checked-out working copy, parses it as a &#x60;DeploymentSpec&#x60;, and upserts   it into the deployment store. - &#x60;ApplySync&#x60; — delegates to [&#x60;apply_sync_inner&#x60;] for the real reconcile.  If a step fails and has an &#x60;on_failure&#x60; handler, that handler runs before the workflow aborts. Subsequent steps are marked &#x60;\&quot;skipped\&quot;&#x60;.  # Errors  Returns [&#x60;ApiError::Forbidden&#x60;] for non-admins, [&#x60;ApiError::NotFound&#x60;] when the workflow does not exist, or [&#x60;ApiError::Internal&#x60;] when execution or recording fails.

### Example

```ts
import {
  Configuration,
  WorkflowsApi,
} from '@zlayer/client';
import type { RunWorkflowRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new WorkflowsApi(config);

  const body = {
    // string | Workflow id
    id: id_example,
  } satisfies RunWorkflowRequest;

  try {
    const data = await api.runWorkflow(body);
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
| **id** | `string` | Workflow id | [Defaults to `undefined`] |

### Return type

[**WorkflowRun**](WorkflowRun.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Workflow run result |  -  |
| **403** | Admin role required |  -  |
| **404** | Workflow not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)

