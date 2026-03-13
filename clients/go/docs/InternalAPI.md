# \InternalAPI

All URIs are relative to *http://localhost*

Method | HTTP request | Description
------------- | ------------- | -------------
[**GetReplicasInternal**](InternalAPI.md#GetReplicasInternal) | **Get** /api/v1/internal/replicas/{service} | Get the current replica count for a service.
[**ScaleServiceInternal**](InternalAPI.md#ScaleServiceInternal) | **Post** /api/v1/internal/scale | Scale a service via internal scheduler request.



## GetReplicasInternal

> InternalScaleResponse GetReplicasInternal(ctx, service).Execute()

Get the current replica count for a service.



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
	service := "service_example" // string | Service name

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.InternalAPI.GetReplicasInternal(context.Background(), service).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `InternalAPI.GetReplicasInternal``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `GetReplicasInternal`: InternalScaleResponse
	fmt.Fprintf(os.Stdout, "Response from `InternalAPI.GetReplicasInternal`: %v\n", resp)
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**service** | **string** | Service name | 

### Other Parameters

Other parameters are passed through a pointer to a apiGetReplicasInternalRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------


### Return type

[**InternalScaleResponse**](InternalScaleResponse.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## ScaleServiceInternal

> InternalScaleResponse ScaleServiceInternal(ctx).InternalScaleRequest(internalScaleRequest).Execute()

Scale a service via internal scheduler request.



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
	internalScaleRequest := *openapiclient.NewInternalScaleRequest(int32(123), "Service_example") // InternalScaleRequest | 

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.InternalAPI.ScaleServiceInternal(context.Background()).InternalScaleRequest(internalScaleRequest).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `InternalAPI.ScaleServiceInternal``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `ScaleServiceInternal`: InternalScaleResponse
	fmt.Fprintf(os.Stdout, "Response from `InternalAPI.ScaleServiceInternal`: %v\n", resp)
}
```

### Path Parameters



### Other Parameters

Other parameters are passed through a pointer to a apiScaleServiceInternalRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
 **internalScaleRequest** | [**InternalScaleRequest**](InternalScaleRequest.md) |  | 

### Return type

[**InternalScaleResponse**](InternalScaleResponse.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: application/json
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)

