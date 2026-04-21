# \CronAPI

All URIs are relative to *http://localhost*

Method | HTTP request | Description
------------- | ------------- | -------------
[**DisableCronJob**](CronAPI.md#DisableCronJob) | **Put** /api/v1/cron/{name}/disable | PUT /api/v1/cron/{name}/disable - Disable a cron job
[**EnableCronJob**](CronAPI.md#EnableCronJob) | **Put** /api/v1/cron/{name}/enable | PUT /api/v1/cron/{name}/enable - Enable a cron job
[**GetCronJob**](CronAPI.md#GetCronJob) | **Get** /api/v1/cron/{name} | GET /api/v1/cron/{name} - Get cron job details
[**ListCronJobs**](CronAPI.md#ListCronJobs) | **Get** /api/v1/cron | GET /api/v1/cron - List all cron jobs
[**TriggerCronJob**](CronAPI.md#TriggerCronJob) | **Post** /api/v1/cron/{name}/trigger | POST /api/v1/cron/{name}/trigger - Manually trigger a cron job



## DisableCronJob

> CronStatusResponse DisableCronJob(ctx, name).Execute()

PUT /api/v1/cron/{name}/disable - Disable a cron job



### Example

```go
package main

import (
	"context"
	"fmt"
	"os"
	openapiclient "github.com/GIT_USER_ID/GIT_REPO_ID"
)

func main() {
	name := "name_example" // string | Cron job name

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.CronAPI.DisableCronJob(context.Background(), name).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `CronAPI.DisableCronJob``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `DisableCronJob`: CronStatusResponse
	fmt.Fprintf(os.Stdout, "Response from `CronAPI.DisableCronJob`: %v\n", resp)
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**name** | **string** | Cron job name | 

### Other Parameters

Other parameters are passed through a pointer to a apiDisableCronJobRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------


### Return type

[**CronStatusResponse**](CronStatusResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## EnableCronJob

> CronStatusResponse EnableCronJob(ctx, name).Execute()

PUT /api/v1/cron/{name}/enable - Enable a cron job



### Example

```go
package main

import (
	"context"
	"fmt"
	"os"
	openapiclient "github.com/GIT_USER_ID/GIT_REPO_ID"
)

func main() {
	name := "name_example" // string | Cron job name

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.CronAPI.EnableCronJob(context.Background(), name).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `CronAPI.EnableCronJob``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `EnableCronJob`: CronStatusResponse
	fmt.Fprintf(os.Stdout, "Response from `CronAPI.EnableCronJob`: %v\n", resp)
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**name** | **string** | Cron job name | 

### Other Parameters

Other parameters are passed through a pointer to a apiEnableCronJobRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------


### Return type

[**CronStatusResponse**](CronStatusResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## GetCronJob

> CronJobResponse GetCronJob(ctx, name).Execute()

GET /api/v1/cron/{name} - Get cron job details



### Example

```go
package main

import (
	"context"
	"fmt"
	"os"
	openapiclient "github.com/GIT_USER_ID/GIT_REPO_ID"
)

func main() {
	name := "name_example" // string | Cron job name

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.CronAPI.GetCronJob(context.Background(), name).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `CronAPI.GetCronJob``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `GetCronJob`: CronJobResponse
	fmt.Fprintf(os.Stdout, "Response from `CronAPI.GetCronJob`: %v\n", resp)
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**name** | **string** | Cron job name | 

### Other Parameters

Other parameters are passed through a pointer to a apiGetCronJobRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------


### Return type

[**CronJobResponse**](CronJobResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## ListCronJobs

> []CronJobResponse ListCronJobs(ctx).Execute()

GET /api/v1/cron - List all cron jobs



### Example

```go
package main

import (
	"context"
	"fmt"
	"os"
	openapiclient "github.com/GIT_USER_ID/GIT_REPO_ID"
)

func main() {

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.CronAPI.ListCronJobs(context.Background()).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `CronAPI.ListCronJobs``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `ListCronJobs`: []CronJobResponse
	fmt.Fprintf(os.Stdout, "Response from `CronAPI.ListCronJobs`: %v\n", resp)
}
```

### Path Parameters

This endpoint does not need any parameter.

### Other Parameters

Other parameters are passed through a pointer to a apiListCronJobsRequest struct via the builder pattern


### Return type

[**[]CronJobResponse**](CronJobResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## TriggerCronJob

> TriggerCronResponse TriggerCronJob(ctx, name).Execute()

POST /api/v1/cron/{name}/trigger - Manually trigger a cron job



### Example

```go
package main

import (
	"context"
	"fmt"
	"os"
	openapiclient "github.com/GIT_USER_ID/GIT_REPO_ID"
)

func main() {
	name := "name_example" // string | Cron job name

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.CronAPI.TriggerCronJob(context.Background(), name).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `CronAPI.TriggerCronJob``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `TriggerCronJob`: TriggerCronResponse
	fmt.Fprintf(os.Stdout, "Response from `CronAPI.TriggerCronJob`: %v\n", resp)
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**name** | **string** | Cron job name | 

### Other Parameters

Other parameters are passed through a pointer to a apiTriggerCronJobRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------


### Return type

[**TriggerCronResponse**](TriggerCronResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)

