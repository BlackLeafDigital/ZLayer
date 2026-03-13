# \ServicesAPI

All URIs are relative to *http://localhost*

Method | HTTP request | Description
------------- | ------------- | -------------
[**GetService**](ServicesAPI.md#GetService) | **Get** /api/v1/deployments/{deployment}/services/{service} | Get service details.
[**GetServiceLogs**](ServicesAPI.md#GetServiceLogs) | **Get** /api/v1/deployments/{deployment}/services/{service}/logs | Get service logs.
[**ListServices**](ServicesAPI.md#ListServices) | **Get** /api/v1/deployments/{deployment}/services | List services in a deployment.
[**ScaleService**](ServicesAPI.md#ScaleService) | **Post** /api/v1/deployments/{deployment}/services/{service}/scale | Scale a service.



## GetService

> ServiceDetails GetService(ctx, deployment, service).Execute()

Get service details.



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
	deployment := "deployment_example" // string | Deployment name
	service := "service_example" // string | Service name

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.ServicesAPI.GetService(context.Background(), deployment, service).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `ServicesAPI.GetService``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `GetService`: ServiceDetails
	fmt.Fprintf(os.Stdout, "Response from `ServicesAPI.GetService`: %v\n", resp)
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**deployment** | **string** | Deployment name | 
**service** | **string** | Service name | 

### Other Parameters

Other parameters are passed through a pointer to a apiGetServiceRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------



### Return type

[**ServiceDetails**](ServiceDetails.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## GetServiceLogs

> string GetServiceLogs(ctx, deployment, service).Lines(lines).Follow(follow).Instance(instance).Execute()

Get service logs.



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
	deployment := "deployment_example" // string | Deployment name
	service := "service_example" // string | Service name
	lines := int32(56) // int32 | Number of lines to return (optional)
	follow := true // bool | Follow logs (streaming) (optional)
	instance := "instance_example" // string | Filter by container/instance (optional)

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.ServicesAPI.GetServiceLogs(context.Background(), deployment, service).Lines(lines).Follow(follow).Instance(instance).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `ServicesAPI.GetServiceLogs``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `GetServiceLogs`: string
	fmt.Fprintf(os.Stdout, "Response from `ServicesAPI.GetServiceLogs`: %v\n", resp)
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**deployment** | **string** | Deployment name | 
**service** | **string** | Service name | 

### Other Parameters

Other parameters are passed through a pointer to a apiGetServiceLogsRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------


 **lines** | **int32** | Number of lines to return | 
 **follow** | **bool** | Follow logs (streaming) | 
 **instance** | **string** | Filter by container/instance | 

### Return type

**string**

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: text/plain

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## ListServices

> []ServiceSummary ListServices(ctx, deployment).Execute()

List services in a deployment.



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
	deployment := "deployment_example" // string | Deployment name

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.ServicesAPI.ListServices(context.Background(), deployment).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `ServicesAPI.ListServices``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `ListServices`: []ServiceSummary
	fmt.Fprintf(os.Stdout, "Response from `ServicesAPI.ListServices`: %v\n", resp)
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**deployment** | **string** | Deployment name | 

### Other Parameters

Other parameters are passed through a pointer to a apiListServicesRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------


### Return type

[**[]ServiceSummary**](ServiceSummary.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## ScaleService

> ServiceDetails ScaleService(ctx, deployment, service).ScaleRequest(scaleRequest).Execute()

Scale a service.



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
	deployment := "deployment_example" // string | Deployment name
	service := "service_example" // string | Service name
	scaleRequest := *openapiclient.NewScaleRequest(int32(123)) // ScaleRequest | 

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.ServicesAPI.ScaleService(context.Background(), deployment, service).ScaleRequest(scaleRequest).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `ServicesAPI.ScaleService``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `ScaleService`: ServiceDetails
	fmt.Fprintf(os.Stdout, "Response from `ServicesAPI.ScaleService`: %v\n", resp)
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**deployment** | **string** | Deployment name | 
**service** | **string** | Service name | 

### Other Parameters

Other parameters are passed through a pointer to a apiScaleServiceRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------


 **scaleRequest** | [**ScaleRequest**](ScaleRequest.md) |  | 

### Return type

[**ServiceDetails**](ServiceDetails.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: application/json
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)

