# \InternalAPI

All URIs are relative to *http://localhost*

Method | HTTP request | Description
------------- | ------------- | -------------
[**GetReplicasInternal**](InternalAPI.md#GetReplicasInternal) | **Get** /api/v1/internal/replicas/{service} | Get the current replica count for a service.
[**InternalRaftTriggerElect**](InternalAPI.md#InternalRaftTriggerElect) | **Post** /api/v1/internal/raft/trigger-elect | Trigger an immediate Raft election on this node.
[**InternalUpgradeStart**](InternalAPI.md#InternalUpgradeStart) | **Post** /api/v1/internal/upgrade/start | Schedule a daemon-binary upgrade on this node.
[**InternalUpgradeStatus**](InternalAPI.md#InternalUpgradeStatus) | **Get** /api/v1/internal/upgrade/{upgrade_id} | Fetch the status of a previously-scheduled daemon-binary upgrade.
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
	openapiclient "github.com/BlackLeafDigital/ZLayer/clients/go"
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


## InternalRaftTriggerElect

> InternalRaftTriggerElect(ctx).Execute()

Trigger an immediate Raft election on this node.



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
	r, err := apiClient.InternalAPI.InternalRaftTriggerElect(context.Background()).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `InternalAPI.InternalRaftTriggerElect``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
}
```

### Path Parameters

This endpoint does not need any parameter.

### Other Parameters

Other parameters are passed through a pointer to a apiInternalRaftTriggerElectRequest struct via the builder pattern


### Return type

 (empty response body)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: Not defined

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## InternalUpgradeStart

> UpgradeStartResponse InternalUpgradeStart(ctx).UpgradeStartRequest(upgradeStartRequest).Execute()

Schedule a daemon-binary upgrade on this node.



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
	upgradeStartRequest := *openapiclient.NewUpgradeStartRequest() // UpgradeStartRequest | 

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.InternalAPI.InternalUpgradeStart(context.Background()).UpgradeStartRequest(upgradeStartRequest).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `InternalAPI.InternalUpgradeStart``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `InternalUpgradeStart`: UpgradeStartResponse
	fmt.Fprintf(os.Stdout, "Response from `InternalAPI.InternalUpgradeStart`: %v\n", resp)
}
```

### Path Parameters



### Other Parameters

Other parameters are passed through a pointer to a apiInternalUpgradeStartRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
 **upgradeStartRequest** | [**UpgradeStartRequest**](UpgradeStartRequest.md) |  | 

### Return type

[**UpgradeStartResponse**](UpgradeStartResponse.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: application/json
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## InternalUpgradeStatus

> UpgradeJobState InternalUpgradeStatus(ctx, upgradeId).Execute()

Fetch the status of a previously-scheduled daemon-binary upgrade.



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
	upgradeId := "upgradeId_example" // string | Upgrade job id returned by internal_upgrade_start

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.InternalAPI.InternalUpgradeStatus(context.Background(), upgradeId).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `InternalAPI.InternalUpgradeStatus``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `InternalUpgradeStatus`: UpgradeJobState
	fmt.Fprintf(os.Stdout, "Response from `InternalAPI.InternalUpgradeStatus`: %v\n", resp)
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**upgradeId** | **string** | Upgrade job id returned by internal_upgrade_start | 

### Other Parameters

Other parameters are passed through a pointer to a apiInternalUpgradeStatusRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------


### Return type

[**UpgradeJobState**](UpgradeJobState.md)

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
	openapiclient "github.com/BlackLeafDigital/ZLayer/clients/go"
)

func main() {
	internalScaleRequest := *openapiclient.NewInternalScaleRequest("Service_example") // InternalScaleRequest | 

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

