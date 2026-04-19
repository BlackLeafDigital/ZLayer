# TasksApi

All URIs are relative to *http://localhost*

| Method | HTTP request | Description |
|------------- | ------------- | -------------|
| [**createTask**](TasksApi.md#createtaskoperation) | **POST** /api/v1/tasks | Create a new task. Admin only. |
| [**deleteTask**](TasksApi.md#deletetask) | **DELETE** /api/v1/tasks/{id} | Delete a task. Admin only. |
| [**getTask**](TasksApi.md#gettask) | **GET** /api/v1/tasks/{id} | Fetch a single task by id. |
| [**listTaskRuns**](TasksApi.md#listtaskruns) | **GET** /api/v1/tasks/{id}/runs | List past runs for a task, most recent first. |
| [**listTasks**](TasksApi.md#listtasks) | **GET** /api/v1/tasks | List tasks. |
| [**runTask**](TasksApi.md#runtask) | **POST** /api/v1/tasks/{id}/run | Execute a task synchronously. Admin only. |



## createTask

> StoredTask createTask(createTaskRequest)

Create a new task. Admin only.

# Errors  Returns [&#x60;ApiError::Forbidden&#x60;] when the caller is not an admin, [&#x60;ApiError::BadRequest&#x60;] for an empty name or body, or [&#x60;ApiError::Internal&#x60;] when the task store fails.

### Example

```ts
import {
  Configuration,
  TasksApi,
} from '@zlayer/client';
import type { CreateTaskOperationRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new TasksApi(config);

  const body = {
    // CreateTaskRequest
    createTaskRequest: ...,
  } satisfies CreateTaskOperationRequest;

  try {
    const data = await api.createTask(body);
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
| **createTaskRequest** | [CreateTaskRequest](CreateTaskRequest.md) |  | |

### Return type

[**StoredTask**](StoredTask.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: `application/json`
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **201** | Task created |  -  |
| **400** | Invalid request |  -  |
| **403** | Admin role required |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## deleteTask

> deleteTask(id)

Delete a task. Admin only.

# Errors  Returns [&#x60;ApiError::Forbidden&#x60;] for non-admins, [&#x60;ApiError::NotFound&#x60;] when the task does not exist, or [&#x60;ApiError::Internal&#x60;] when the store fails.

### Example

```ts
import {
  Configuration,
  TasksApi,
} from '@zlayer/client';
import type { DeleteTaskRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new TasksApi(config);

  const body = {
    // string | Task id
    id: id_example,
  } satisfies DeleteTaskRequest;

  try {
    const data = await api.deleteTask(body);
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
| **id** | `string` | Task id | [Defaults to `undefined`] |

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
| **204** | Task deleted |  -  |
| **403** | Admin role required |  -  |
| **404** | Not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## getTask

> StoredTask getTask(id)

Fetch a single task by id.

# Errors  Returns [&#x60;ApiError::NotFound&#x60;] if no task with the given id exists, or [&#x60;ApiError::Internal&#x60;] when the store fails.

### Example

```ts
import {
  Configuration,
  TasksApi,
} from '@zlayer/client';
import type { GetTaskRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new TasksApi(config);

  const body = {
    // string | Task id
    id: id_example,
  } satisfies GetTaskRequest;

  try {
    const data = await api.getTask(body);
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
| **id** | `string` | Task id | [Defaults to `undefined`] |

### Return type

[**StoredTask**](StoredTask.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Task |  -  |
| **404** | Not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## listTaskRuns

> Array&lt;TaskRun&gt; listTaskRuns(id)

List past runs for a task, most recent first.

# Errors  Returns [&#x60;ApiError::NotFound&#x60;] if the task does not exist, or [&#x60;ApiError::Internal&#x60;] when the store fails.

### Example

```ts
import {
  Configuration,
  TasksApi,
} from '@zlayer/client';
import type { ListTaskRunsRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new TasksApi(config);

  const body = {
    // string | Task id
    id: id_example,
  } satisfies ListTaskRunsRequest;

  try {
    const data = await api.listTaskRuns(body);
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
| **id** | `string` | Task id | [Defaults to `undefined`] |

### Return type

[**Array&lt;TaskRun&gt;**](TaskRun.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | List of task runs |  -  |
| **404** | Task not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## listTasks

> Array&lt;StoredTask&gt; listTasks(projectId)

List tasks.

# Errors  Returns [&#x60;ApiError::Internal&#x60;] if the task store fails.

### Example

```ts
import {
  Configuration,
  TasksApi,
} from '@zlayer/client';
import type { ListTasksRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new TasksApi(config);

  const body = {
    // string | Filter by project id; omit to list all tasks (optional)
    projectId: projectId_example,
  } satisfies ListTasksRequest;

  try {
    const data = await api.listTasks(body);
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
| **projectId** | `string` | Filter by project id; omit to list all tasks | [Optional] [Defaults to `undefined`] |

### Return type

[**Array&lt;StoredTask&gt;**](StoredTask.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | List of tasks |  -  |
| **401** | Unauthorized |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## runTask

> TaskRun runTask(id)

Execute a task synchronously. Admin only.

Runs the task\&#39;s body via &#x60;sh -c&#x60; on Unix, captures stdout/stderr, and returns the completed &#x60;TaskRun&#x60;.  # Errors  Returns [&#x60;ApiError::Forbidden&#x60;] for non-admins, [&#x60;ApiError::NotFound&#x60;] when the task does not exist, [&#x60;ApiError::NotImplemented&#x60;] on non-Unix platforms, or [&#x60;ApiError::Internal&#x60;] when the store or execution fails.

### Example

```ts
import {
  Configuration,
  TasksApi,
} from '@zlayer/client';
import type { RunTaskRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new TasksApi(config);

  const body = {
    // string | Task id
    id: id_example,
  } satisfies RunTaskRequest;

  try {
    const data = await api.runTask(body);
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
| **id** | `string` | Task id | [Defaults to `undefined`] |

### Return type

[**TaskRun**](TaskRun.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Task run result |  -  |
| **403** | Admin role required |  -  |
| **404** | Task not found |  -  |
| **501** | Not implemented on this platform |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)

