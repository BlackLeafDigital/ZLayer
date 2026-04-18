# JobsApi

All URIs are relative to *http://localhost*

| Method | HTTP request | Description |
|------------- | ------------- | -------------|
| [**cancelExecution**](JobsApi.md#cancelexecution) | **POST** /api/v1/jobs/{execution_id}/cancel | POST /&#x60;api/v1/jobs/{execution_id}/cancel&#x60; - Cancel a running execution |
| [**getExecutionStatus**](JobsApi.md#getexecutionstatus) | **GET** /api/v1/jobs/{execution_id}/status | GET /&#x60;api/v1/jobs/{execution_id}/status&#x60; - Get execution status |
| [**listJobExecutions**](JobsApi.md#listjobexecutions) | **GET** /api/v1/jobs/{name}/executions | GET /api/v1/jobs/{name}/executions - List executions for a job |
| [**triggerJob**](JobsApi.md#triggerjob) | **POST** /api/v1/jobs/{name}/trigger | POST /api/v1/jobs/{name}/trigger - Trigger a job execution |



## cancelExecution

> cancelExecution(executionId)

POST /&#x60;api/v1/jobs/{execution_id}/cancel&#x60; - Cancel a running execution

Attempts to cancel a running or pending job execution.  # Errors  Returns an error if the execution is not found, already completed, or cancellation fails.

### Example

```ts
import {
  Configuration,
  JobsApi,
} from '@zlayer/client';
import type { CancelExecutionRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new JobsApi(config);

  const body = {
    // string | Execution ID
    executionId: executionId_example,
  } satisfies CancelExecutionRequest;

  try {
    const data = await api.cancelExecution(body);
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
| **executionId** | `string` | Execution ID | [Defaults to `undefined`] |

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
| **200** | Execution cancelled |  -  |
| **401** | Unauthorized |  -  |
| **404** | Execution not found |  -  |
| **409** | Execution already completed |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## getExecutionStatus

> JobExecutionResponse getExecutionStatus(executionId)

GET /&#x60;api/v1/jobs/{execution_id}/status&#x60; - Get execution status

Returns the current status of a job execution, including logs if available.  # Errors  Returns an error if the execution is not found.

### Example

```ts
import {
  Configuration,
  JobsApi,
} from '@zlayer/client';
import type { GetExecutionStatusRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new JobsApi(config);

  const body = {
    // string | Execution ID
    executionId: executionId_example,
  } satisfies GetExecutionStatusRequest;

  try {
    const data = await api.getExecutionStatus(body);
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
| **executionId** | `string` | Execution ID | [Defaults to `undefined`] |

### Return type

[**JobExecutionResponse**](JobExecutionResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Execution status |  -  |
| **401** | Unauthorized |  -  |
| **404** | Execution not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## listJobExecutions

> Array&lt;JobExecutionResponse&gt; listJobExecutions(name, limit, status)

GET /api/v1/jobs/{name}/executions - List executions for a job

Returns a list of recent executions for the specified job.  # Errors  Returns an error if authentication fails.

### Example

```ts
import {
  Configuration,
  JobsApi,
} from '@zlayer/client';
import type { ListJobExecutionsRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new JobsApi(config);

  const body = {
    // string | Job name
    name: name_example,
    // number | Maximum number of executions to return (optional)
    limit: 56,
    // string | Filter by status (pending, running, completed, failed) (optional)
    status: status_example,
  } satisfies ListJobExecutionsRequest;

  try {
    const data = await api.listJobExecutions(body);
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
| **name** | `string` | Job name | [Defaults to `undefined`] |
| **limit** | `number` | Maximum number of executions to return | [Optional] [Defaults to `undefined`] |
| **status** | `string` | Filter by status (pending, running, completed, failed) | [Optional] [Defaults to `undefined`] |

### Return type

[**Array&lt;JobExecutionResponse&gt;**](JobExecutionResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | List of executions |  -  |
| **401** | Unauthorized |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## triggerJob

> TriggerJobResponse triggerJob(name)

POST /api/v1/jobs/{name}/trigger - Trigger a job execution

Starts a new execution of the specified job. Returns immediately with an execution ID that can be used to track the job\&#39;s progress.  # Errors  Returns an error if the job is not found or triggering fails.

### Example

```ts
import {
  Configuration,
  JobsApi,
} from '@zlayer/client';
import type { TriggerJobRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new JobsApi(config);

  const body = {
    // string | Job name
    name: name_example,
  } satisfies TriggerJobRequest;

  try {
    const data = await api.triggerJob(body);
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
| **name** | `string` | Job name | [Defaults to `undefined`] |

### Return type

[**TriggerJobResponse**](TriggerJobResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **202** | Job triggered successfully |  -  |
| **401** | Unauthorized |  -  |
| **404** | Job not found |  -  |
| **500** | Internal error |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)

