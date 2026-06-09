# \WorkflowsAPI

All URIs are relative to *http://localhost*

Method | HTTP request | Description
------------- | ------------- | -------------
[**CreateWorkflow**](WorkflowsAPI.md#CreateWorkflow) | **Post** /api/v1/workflows | Create a new workflow. Admin only.
[**DeleteWorkflow**](WorkflowsAPI.md#DeleteWorkflow) | **Delete** /api/v1/workflows/{id} | Delete a workflow. Admin only.
[**GetWorkflow**](WorkflowsAPI.md#GetWorkflow) | **Get** /api/v1/workflows/{id} | Fetch a single workflow by id.
[**ListWorkflowRuns**](WorkflowsAPI.md#ListWorkflowRuns) | **Get** /api/v1/workflows/{id}/runs | List past runs for a workflow, most recent first.
[**ListWorkflows**](WorkflowsAPI.md#ListWorkflows) | **Get** /api/v1/workflows | List workflows.
[**RunWorkflow**](WorkflowsAPI.md#RunWorkflow) | **Post** /api/v1/workflows/{id}/run | Execute a workflow synchronously. Admin only.



## CreateWorkflow

> StoredWorkflow CreateWorkflow(ctx).CreateWorkflowRequest(createWorkflowRequest).Execute()

Create a new workflow. Admin only.



### Example

```go
package main

import (
	"context"
	"fmt"
	"os"
	openapiclient "github.com/BlackLeafDigital/ZLayer/clients/go"
)

func main() {
	createWorkflowRequest := *openapiclient.NewCreateWorkflowRequest("Name_example", []openapiclient.WorkflowStep{*openapiclient.NewWorkflowStep(openapiclient.WorkflowAction{WorkflowActionOneOf: openapiclient.NewWorkflowActionOneOf("TaskId_example", "Type_example")}, "Name_example")}) // CreateWorkflowRequest | 

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.WorkflowsAPI.CreateWorkflow(context.Background()).CreateWorkflowRequest(createWorkflowRequest).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `WorkflowsAPI.CreateWorkflow``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `CreateWorkflow`: StoredWorkflow
	fmt.Fprintf(os.Stdout, "Response from `WorkflowsAPI.CreateWorkflow`: %v\n", resp)
}
```

### Path Parameters



### Other Parameters

Other parameters are passed through a pointer to a apiCreateWorkflowRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
 **createWorkflowRequest** | [**CreateWorkflowRequest**](CreateWorkflowRequest.md) |  | 

### Return type

[**StoredWorkflow**](StoredWorkflow.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: application/json
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## DeleteWorkflow

> DeleteWorkflow(ctx, id).Execute()

Delete a workflow. Admin only.



### Example

```go
package main

import (
	"context"
	"fmt"
	"os"
	openapiclient "github.com/BlackLeafDigital/ZLayer/clients/go"
)

func main() {
	id := "id_example" // string | Workflow id

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	r, err := apiClient.WorkflowsAPI.DeleteWorkflow(context.Background(), id).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `WorkflowsAPI.DeleteWorkflow``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**id** | **string** | Workflow id | 

### Other Parameters

Other parameters are passed through a pointer to a apiDeleteWorkflowRequest struct via the builder pattern


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


## GetWorkflow

> StoredWorkflow GetWorkflow(ctx, id).Execute()

Fetch a single workflow by id.



### Example

```go
package main

import (
	"context"
	"fmt"
	"os"
	openapiclient "github.com/BlackLeafDigital/ZLayer/clients/go"
)

func main() {
	id := "id_example" // string | Workflow id

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.WorkflowsAPI.GetWorkflow(context.Background(), id).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `WorkflowsAPI.GetWorkflow``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `GetWorkflow`: StoredWorkflow
	fmt.Fprintf(os.Stdout, "Response from `WorkflowsAPI.GetWorkflow`: %v\n", resp)
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**id** | **string** | Workflow id | 

### Other Parameters

Other parameters are passed through a pointer to a apiGetWorkflowRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------


### Return type

[**StoredWorkflow**](StoredWorkflow.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## ListWorkflowRuns

> []WorkflowRun ListWorkflowRuns(ctx, id).Execute()

List past runs for a workflow, most recent first.



### Example

```go
package main

import (
	"context"
	"fmt"
	"os"
	openapiclient "github.com/BlackLeafDigital/ZLayer/clients/go"
)

func main() {
	id := "id_example" // string | Workflow id

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.WorkflowsAPI.ListWorkflowRuns(context.Background(), id).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `WorkflowsAPI.ListWorkflowRuns``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `ListWorkflowRuns`: []WorkflowRun
	fmt.Fprintf(os.Stdout, "Response from `WorkflowsAPI.ListWorkflowRuns`: %v\n", resp)
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**id** | **string** | Workflow id | 

### Other Parameters

Other parameters are passed through a pointer to a apiListWorkflowRunsRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------


### Return type

[**[]WorkflowRun**](WorkflowRun.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## ListWorkflows

> []StoredWorkflow ListWorkflows(ctx).Execute()

List workflows.



### Example

```go
package main

import (
	"context"
	"fmt"
	"os"
	openapiclient "github.com/BlackLeafDigital/ZLayer/clients/go"
)

func main() {

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.WorkflowsAPI.ListWorkflows(context.Background()).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `WorkflowsAPI.ListWorkflows``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `ListWorkflows`: []StoredWorkflow
	fmt.Fprintf(os.Stdout, "Response from `WorkflowsAPI.ListWorkflows`: %v\n", resp)
}
```

### Path Parameters

This endpoint does not need any parameter.

### Other Parameters

Other parameters are passed through a pointer to a apiListWorkflowsRequest struct via the builder pattern


### Return type

[**[]StoredWorkflow**](StoredWorkflow.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## RunWorkflow

> WorkflowRun RunWorkflow(ctx, id).Execute()

Execute a workflow synchronously. Admin only.



### Example

```go
package main

import (
	"context"
	"fmt"
	"os"
	openapiclient "github.com/BlackLeafDigital/ZLayer/clients/go"
)

func main() {
	id := "id_example" // string | Workflow id

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.WorkflowsAPI.RunWorkflow(context.Background(), id).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `WorkflowsAPI.RunWorkflow``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `RunWorkflow`: WorkflowRun
	fmt.Fprintf(os.Stdout, "Response from `WorkflowsAPI.RunWorkflow`: %v\n", resp)
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**id** | **string** | Workflow id | 

### Other Parameters

Other parameters are passed through a pointer to a apiRunWorkflowRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------


### Return type

[**WorkflowRun**](WorkflowRun.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)

