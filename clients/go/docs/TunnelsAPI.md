# \TunnelsAPI

All URIs are relative to *http://localhost*

Method | HTTP request | Description
------------- | ------------- | -------------
[**CreateNodeTunnel**](TunnelsAPI.md#CreateNodeTunnel) | **Post** /api/v1/tunnels/node | Create a node-to-node tunnel.
[**CreateTunnel**](TunnelsAPI.md#CreateTunnel) | **Post** /api/v1/tunnels | Create a new tunnel token.
[**GetTunnelStatus**](TunnelsAPI.md#GetTunnelStatus) | **Get** /api/v1/tunnels/{id}/status | Get tunnel status.
[**ListTunnels**](TunnelsAPI.md#ListTunnels) | **Get** /api/v1/tunnels | List all tunnels.
[**RemoveNodeTunnel**](TunnelsAPI.md#RemoveNodeTunnel) | **Delete** /api/v1/tunnels/node/{name} | Remove a node-to-node tunnel.
[**RevokeTunnel**](TunnelsAPI.md#RevokeTunnel) | **Delete** /api/v1/tunnels/{id} | Revoke (delete) a tunnel.



## CreateNodeTunnel

> CreateNodeTunnelResponse CreateNodeTunnel(ctx).CreateNodeTunnelRequest(createNodeTunnelRequest).Execute()

Create a node-to-node tunnel.



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
	createNodeTunnelRequest := *openapiclient.NewCreateNodeTunnelRequest("FromNode_example", int32(123), "Name_example", int32(123), "ToNode_example") // CreateNodeTunnelRequest | 

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.TunnelsAPI.CreateNodeTunnel(context.Background()).CreateNodeTunnelRequest(createNodeTunnelRequest).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `TunnelsAPI.CreateNodeTunnel``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `CreateNodeTunnel`: CreateNodeTunnelResponse
	fmt.Fprintf(os.Stdout, "Response from `TunnelsAPI.CreateNodeTunnel`: %v\n", resp)
}
```

### Path Parameters



### Other Parameters

Other parameters are passed through a pointer to a apiCreateNodeTunnelRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
 **createNodeTunnelRequest** | [**CreateNodeTunnelRequest**](CreateNodeTunnelRequest.md) |  | 

### Return type

[**CreateNodeTunnelResponse**](CreateNodeTunnelResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: application/json
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## CreateTunnel

> CreateTunnelResponse CreateTunnel(ctx).CreateTunnelRequest(createTunnelRequest).Execute()

Create a new tunnel token.



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
	createTunnelRequest := *openapiclient.NewCreateTunnelRequest("Name_example") // CreateTunnelRequest | 

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.TunnelsAPI.CreateTunnel(context.Background()).CreateTunnelRequest(createTunnelRequest).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `TunnelsAPI.CreateTunnel``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `CreateTunnel`: CreateTunnelResponse
	fmt.Fprintf(os.Stdout, "Response from `TunnelsAPI.CreateTunnel`: %v\n", resp)
}
```

### Path Parameters



### Other Parameters

Other parameters are passed through a pointer to a apiCreateTunnelRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
 **createTunnelRequest** | [**CreateTunnelRequest**](CreateTunnelRequest.md) |  | 

### Return type

[**CreateTunnelResponse**](CreateTunnelResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: application/json
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## GetTunnelStatus

> TunnelStatus GetTunnelStatus(ctx, id).Execute()

Get tunnel status.



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
	id := "id_example" // string | Tunnel identifier

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.TunnelsAPI.GetTunnelStatus(context.Background(), id).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `TunnelsAPI.GetTunnelStatus``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `GetTunnelStatus`: TunnelStatus
	fmt.Fprintf(os.Stdout, "Response from `TunnelsAPI.GetTunnelStatus`: %v\n", resp)
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**id** | **string** | Tunnel identifier | 

### Other Parameters

Other parameters are passed through a pointer to a apiGetTunnelStatusRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------


### Return type

[**TunnelStatus**](TunnelStatus.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## ListTunnels

> []TunnelSummary ListTunnels(ctx).Execute()

List all tunnels.



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
	resp, r, err := apiClient.TunnelsAPI.ListTunnels(context.Background()).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `TunnelsAPI.ListTunnels``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `ListTunnels`: []TunnelSummary
	fmt.Fprintf(os.Stdout, "Response from `TunnelsAPI.ListTunnels`: %v\n", resp)
}
```

### Path Parameters

This endpoint does not need any parameter.

### Other Parameters

Other parameters are passed through a pointer to a apiListTunnelsRequest struct via the builder pattern


### Return type

[**[]TunnelSummary**](TunnelSummary.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## RemoveNodeTunnel

> SuccessResponse RemoveNodeTunnel(ctx, name).Execute()

Remove a node-to-node tunnel.



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
	name := "name_example" // string | Tunnel name

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.TunnelsAPI.RemoveNodeTunnel(context.Background(), name).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `TunnelsAPI.RemoveNodeTunnel``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `RemoveNodeTunnel`: SuccessResponse
	fmt.Fprintf(os.Stdout, "Response from `TunnelsAPI.RemoveNodeTunnel`: %v\n", resp)
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**name** | **string** | Tunnel name | 

### Other Parameters

Other parameters are passed through a pointer to a apiRemoveNodeTunnelRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------


### Return type

[**SuccessResponse**](SuccessResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## RevokeTunnel

> SuccessResponse RevokeTunnel(ctx, id).Execute()

Revoke (delete) a tunnel.



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
	id := "id_example" // string | Tunnel identifier

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.TunnelsAPI.RevokeTunnel(context.Background(), id).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `TunnelsAPI.RevokeTunnel``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `RevokeTunnel`: SuccessResponse
	fmt.Fprintf(os.Stdout, "Response from `TunnelsAPI.RevokeTunnel`: %v\n", resp)
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**id** | **string** | Tunnel identifier | 

### Other Parameters

Other parameters are passed through a pointer to a apiRevokeTunnelRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------


### Return type

[**SuccessResponse**](SuccessResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)

