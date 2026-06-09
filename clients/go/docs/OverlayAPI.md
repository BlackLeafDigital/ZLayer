# \OverlayAPI

All URIs are relative to *http://localhost*

Method | HTTP request | Description
------------- | ------------- | -------------
[**GetDnsStatus**](OverlayAPI.md#GetDnsStatus) | **Get** /api/v1/overlay/dns | Get DNS service status.
[**GetIpAllocation**](OverlayAPI.md#GetIpAllocation) | **Get** /api/v1/overlay/ip-alloc | Get IP allocation status.
[**GetNatStatus**](OverlayAPI.md#GetNatStatus) | **Get** /api/v1/overlay/nat/status | Get NAT traversal status.
[**GetOverlayPeers**](OverlayAPI.md#GetOverlayPeers) | **Get** /api/v1/overlay/peers | Get list of overlay peers.
[**GetOverlayStatus**](OverlayAPI.md#GetOverlayStatus) | **Get** /api/v1/overlay/status | Get overlay network status.
[**GetServiceBridge**](OverlayAPI.md#GetServiceBridge) | **Get** /api/v1/overlay/services/{name}/bridges/{node_id} | Get the bridge attachment for a service on a specific node.
[**GetServiceOverlayStatus**](OverlayAPI.md#GetServiceOverlayStatus) | **Get** /api/v1/overlay/services/{name} | Get the overlay status for a single service.



## GetDnsStatus

> DnsStatusResponse GetDnsStatus(ctx).Execute()

Get DNS service status.



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
	resp, r, err := apiClient.OverlayAPI.GetDnsStatus(context.Background()).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `OverlayAPI.GetDnsStatus``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `GetDnsStatus`: DnsStatusResponse
	fmt.Fprintf(os.Stdout, "Response from `OverlayAPI.GetDnsStatus`: %v\n", resp)
}
```

### Path Parameters

This endpoint does not need any parameter.

### Other Parameters

Other parameters are passed through a pointer to a apiGetDnsStatusRequest struct via the builder pattern


### Return type

[**DnsStatusResponse**](DnsStatusResponse.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## GetIpAllocation

> IpAllocationResponse GetIpAllocation(ctx).Execute()

Get IP allocation status.



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
	resp, r, err := apiClient.OverlayAPI.GetIpAllocation(context.Background()).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `OverlayAPI.GetIpAllocation``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `GetIpAllocation`: IpAllocationResponse
	fmt.Fprintf(os.Stdout, "Response from `OverlayAPI.GetIpAllocation`: %v\n", resp)
}
```

### Path Parameters

This endpoint does not need any parameter.

### Other Parameters

Other parameters are passed through a pointer to a apiGetIpAllocationRequest struct via the builder pattern


### Return type

[**IpAllocationResponse**](IpAllocationResponse.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## GetNatStatus

> NatStatusResponse GetNatStatus(ctx).Execute()

Get NAT traversal status.



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
	resp, r, err := apiClient.OverlayAPI.GetNatStatus(context.Background()).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `OverlayAPI.GetNatStatus``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `GetNatStatus`: NatStatusResponse
	fmt.Fprintf(os.Stdout, "Response from `OverlayAPI.GetNatStatus`: %v\n", resp)
}
```

### Path Parameters

This endpoint does not need any parameter.

### Other Parameters

Other parameters are passed through a pointer to a apiGetNatStatusRequest struct via the builder pattern


### Return type

[**NatStatusResponse**](NatStatusResponse.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## GetOverlayPeers

> PeerListResponse GetOverlayPeers(ctx).Execute()

Get list of overlay peers.



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
	resp, r, err := apiClient.OverlayAPI.GetOverlayPeers(context.Background()).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `OverlayAPI.GetOverlayPeers``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `GetOverlayPeers`: PeerListResponse
	fmt.Fprintf(os.Stdout, "Response from `OverlayAPI.GetOverlayPeers`: %v\n", resp)
}
```

### Path Parameters

This endpoint does not need any parameter.

### Other Parameters

Other parameters are passed through a pointer to a apiGetOverlayPeersRequest struct via the builder pattern


### Return type

[**PeerListResponse**](PeerListResponse.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## GetOverlayStatus

> OverlayStatusResponse GetOverlayStatus(ctx).Execute()

Get overlay network status.



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
	resp, r, err := apiClient.OverlayAPI.GetOverlayStatus(context.Background()).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `OverlayAPI.GetOverlayStatus``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `GetOverlayStatus`: OverlayStatusResponse
	fmt.Fprintf(os.Stdout, "Response from `OverlayAPI.GetOverlayStatus`: %v\n", resp)
}
```

### Path Parameters

This endpoint does not need any parameter.

### Other Parameters

Other parameters are passed through a pointer to a apiGetOverlayStatusRequest struct via the builder pattern


### Return type

[**OverlayStatusResponse**](OverlayStatusResponse.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## GetServiceBridge

> BridgeInfo GetServiceBridge(ctx, name, nodeId).Execute()

Get the bridge attachment for a service on a specific node.



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
	name := "name_example" // string | Service name
	nodeId := "nodeId_example" // string | Node ID

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.OverlayAPI.GetServiceBridge(context.Background(), name, nodeId).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `OverlayAPI.GetServiceBridge``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `GetServiceBridge`: BridgeInfo
	fmt.Fprintf(os.Stdout, "Response from `OverlayAPI.GetServiceBridge`: %v\n", resp)
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**name** | **string** | Service name | 
**nodeId** | **string** | Node ID | 

### Other Parameters

Other parameters are passed through a pointer to a apiGetServiceBridgeRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------



### Return type

[**BridgeInfo**](BridgeInfo.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## GetServiceOverlayStatus

> ServiceOverlayStatus GetServiceOverlayStatus(ctx, name).Execute()

Get the overlay status for a single service.



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
	name := "name_example" // string | Service name

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.OverlayAPI.GetServiceOverlayStatus(context.Background(), name).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `OverlayAPI.GetServiceOverlayStatus``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `GetServiceOverlayStatus`: ServiceOverlayStatus
	fmt.Fprintf(os.Stdout, "Response from `OverlayAPI.GetServiceOverlayStatus`: %v\n", resp)
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**name** | **string** | Service name | 

### Other Parameters

Other parameters are passed through a pointer to a apiGetServiceOverlayStatusRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------


### Return type

[**ServiceOverlayStatus**](ServiceOverlayStatus.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)

