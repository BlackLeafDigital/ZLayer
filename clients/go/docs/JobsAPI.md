# \JobsAPI

All URIs are relative to *http://localhost*

Method | HTTP request | Description
------------- | ------------- | -------------
[**CancelExecution**](JobsAPI.md#CancelExecution) | **Post** /api/v1/jobs/{execution_id}/cancel | POST /&#x60;api/v1/jobs/{execution_id}/cancel&#x60; - Cancel a running execution
[**GetExecutionStatus**](JobsAPI.md#GetExecutionStatus) | **Get** /api/v1/jobs/{execution_id}/status | GET /&#x60;api/v1/jobs/{execution_id}/status&#x60; - Get execution status
[**ListJobExecutions**](JobsAPI.md#ListJobExecutions) | **Get** /api/v1/jobs/{name}/executions | GET /api/v1/jobs/{name}/executions - List executions for a job
[**TriggerJob**](JobsAPI.md#TriggerJob) | **Post** /api/v1/jobs/{name}/trigger | POST /api/v1/jobs/{name}/trigger - Trigger a job execution



## CancelExecution

> CancelExecution(ctx, executionId).Execute()

POST /`api/v1/jobs/{execution_id}/cancel` - Cancel a running execution



### Example

```go
package main

import (
	"context"
	"fmt"
	"os"
	openapiclient "github.com/GIT_USER_ID/GIT_REPO_ID/zlayer"
)

func main() {
	executionId := "executionId_example" // string | Execution ID

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	r, err := apiClient.JobsAPI.CancelExecution(context.Background(), executionId).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `JobsAPI.CancelExecution``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**executionId** | **string** | Execution ID | 

### Other Parameters

Other parameters are passed through a pointer to a apiCancelExecutionRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------


### Return type

 (empty response body)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: Not defined

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## GetExecutionStatus

> JobExecutionResponse GetExecutionStatus(ctx, executionId).Execute()

GET /`api/v1/jobs/{execution_id}/status` - Get execution status



### Example

```go
package main

import (
	"context"
	"fmt"
	"os"
	openapiclient "github.com/GIT_USER_ID/GIT_REPO_ID/zlayer"
)

func main() {
	executionId := "executionId_example" // string | Execution ID

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.JobsAPI.GetExecutionStatus(context.Background(), executionId).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `JobsAPI.GetExecutionStatus``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `GetExecutionStatus`: JobExecutionResponse
	fmt.Fprintf(os.Stdout, "Response from `JobsAPI.GetExecutionStatus`: %v\n", resp)
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**executionId** | **string** | Execution ID | 

### Other Parameters

Other parameters are passed through a pointer to a apiGetExecutionStatusRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------


### Return type

[**JobExecutionResponse**](JobExecutionResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## ListJobExecutions

> []JobExecutionResponse ListJobExecutions(ctx, name).Limit(limit).Status(status).Execute()

GET /api/v1/jobs/{name}/executions - List executions for a job



### Example

```go
package main

import (
	"context"
	"fmt"
	"os"
	openapiclient "github.com/GIT_USER_ID/GIT_REPO_ID/zlayer"
)

func main() {
	name := "name_example" // string | Job name
	limit := int32(56) // int32 | Maximum number of executions to return (optional)
	status := "status_example" // string | Filter by status (pending, running, completed, failed) (optional)

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.JobsAPI.ListJobExecutions(context.Background(), name).Limit(limit).Status(status).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `JobsAPI.ListJobExecutions``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `ListJobExecutions`: []JobExecutionResponse
	fmt.Fprintf(os.Stdout, "Response from `JobsAPI.ListJobExecutions`: %v\n", resp)
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**name** | **string** | Job name | 

### Other Parameters

Other parameters are passed through a pointer to a apiListJobExecutionsRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------

 **limit** | **int32** | Maximum number of executions to return | 
 **status** | **string** | Filter by status (pending, running, completed, failed) | 

### Return type

[**[]JobExecutionResponse**](JobExecutionResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## TriggerJob

> TriggerJobResponse TriggerJob(ctx, name).Execute()

POST /api/v1/jobs/{name}/trigger - Trigger a job execution



### Example

```go
package main

import (
	"context"
	"fmt"
	"os"
	openapiclient "github.com/GIT_USER_ID/GIT_REPO_ID/zlayer"
)

func main() {
	name := "name_example" // string | Job name

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.JobsAPI.TriggerJob(context.Background(), name).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `JobsAPI.TriggerJob``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `TriggerJob`: TriggerJobResponse
	fmt.Fprintf(os.Stdout, "Response from `JobsAPI.TriggerJob`: %v\n", resp)
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**name** | **string** | Job name | 

### Other Parameters

Other parameters are passed through a pointer to a apiTriggerJobRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------


### Return type

[**TriggerJobResponse**](TriggerJobResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)

