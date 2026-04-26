# \ContainerNetworksAPI

All URIs are relative to *http://localhost*

Method | HTTP request | Description
------------- | ------------- | -------------
[**ConnectContainerNetwork**](ContainerNetworksAPI.md#ConnectContainerNetwork) | **Post** /api/v1/container-networks/{id_or_name}/connect | Attach a container to a network.
[**CreateContainerNetwork**](ContainerNetworksAPI.md#CreateContainerNetwork) | **Post** /api/v1/container-networks | Create a new bridge or overlay network.
[**DeleteContainerNetwork**](ContainerNetworksAPI.md#DeleteContainerNetwork) | **Delete** /api/v1/container-networks/{id_or_name} | Delete a bridge network. Refuses if the network still has attachments unless &#x60;?force&#x3D;true&#x60; is set.
[**DisconnectContainerNetwork**](ContainerNetworksAPI.md#DisconnectContainerNetwork) | **Post** /api/v1/container-networks/{id_or_name}/disconnect | Detach a container from a network.
[**GetContainerNetwork**](ContainerNetworksAPI.md#GetContainerNetwork) | **Get** /api/v1/container-networks/{id_or_name} | Inspect a single bridge network by id or by name.
[**ListContainerNetworks**](ContainerNetworksAPI.md#ListContainerNetworks) | **Get** /api/v1/container-networks | List all bridge networks, optionally filtered by label.



## ConnectContainerNetwork

> ConnectContainerNetwork(ctx, idOrName).ConnectBridgeNetworkRequest(connectBridgeNetworkRequest).Execute()

Attach a container to a network.



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
	idOrName := "idOrName_example" // string | Network id or name
	connectBridgeNetworkRequest := *openapiclient.NewConnectBridgeNetworkRequest("ContainerId_example") // ConnectBridgeNetworkRequest | 

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	r, err := apiClient.ContainerNetworksAPI.ConnectContainerNetwork(context.Background(), idOrName).ConnectBridgeNetworkRequest(connectBridgeNetworkRequest).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `ContainerNetworksAPI.ConnectContainerNetwork``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**idOrName** | **string** | Network id or name | 

### Other Parameters

Other parameters are passed through a pointer to a apiConnectContainerNetworkRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------

 **connectBridgeNetworkRequest** | [**ConnectBridgeNetworkRequest**](ConnectBridgeNetworkRequest.md) |  | 

### Return type

 (empty response body)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: application/json
- **Accept**: Not defined

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## CreateContainerNetwork

> BridgeNetwork CreateContainerNetwork(ctx).CreateBridgeNetworkRequest(createBridgeNetworkRequest).Execute()

Create a new bridge or overlay network.



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
	createBridgeNetworkRequest := *openapiclient.NewCreateBridgeNetworkRequest("Name_example") // CreateBridgeNetworkRequest | 

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.ContainerNetworksAPI.CreateContainerNetwork(context.Background()).CreateBridgeNetworkRequest(createBridgeNetworkRequest).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `ContainerNetworksAPI.CreateContainerNetwork``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `CreateContainerNetwork`: BridgeNetwork
	fmt.Fprintf(os.Stdout, "Response from `ContainerNetworksAPI.CreateContainerNetwork`: %v\n", resp)
}
```

### Path Parameters



### Other Parameters

Other parameters are passed through a pointer to a apiCreateContainerNetworkRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
 **createBridgeNetworkRequest** | [**CreateBridgeNetworkRequest**](CreateBridgeNetworkRequest.md) |  | 

### Return type

[**BridgeNetwork**](BridgeNetwork.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: application/json
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## DeleteContainerNetwork

> DeleteContainerNetwork(ctx, idOrName).Force(force).Execute()

Delete a bridge network. Refuses if the network still has attachments unless `?force=true` is set.



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
	idOrName := "idOrName_example" // string | Network id or name
	force := true // bool | If true, delete even if the network still has attachments. (optional)

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	r, err := apiClient.ContainerNetworksAPI.DeleteContainerNetwork(context.Background(), idOrName).Force(force).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `ContainerNetworksAPI.DeleteContainerNetwork``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**idOrName** | **string** | Network id or name | 

### Other Parameters

Other parameters are passed through a pointer to a apiDeleteContainerNetworkRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------

 **force** | **bool** | If true, delete even if the network still has attachments. | 

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


## DisconnectContainerNetwork

> DisconnectContainerNetwork(ctx, idOrName).DisconnectBridgeNetworkRequest(disconnectBridgeNetworkRequest).Execute()

Detach a container from a network.



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
	idOrName := "idOrName_example" // string | Network id or name
	disconnectBridgeNetworkRequest := *openapiclient.NewDisconnectBridgeNetworkRequest("ContainerId_example") // DisconnectBridgeNetworkRequest | 

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	r, err := apiClient.ContainerNetworksAPI.DisconnectContainerNetwork(context.Background(), idOrName).DisconnectBridgeNetworkRequest(disconnectBridgeNetworkRequest).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `ContainerNetworksAPI.DisconnectContainerNetwork``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**idOrName** | **string** | Network id or name | 

### Other Parameters

Other parameters are passed through a pointer to a apiDisconnectContainerNetworkRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------

 **disconnectBridgeNetworkRequest** | [**DisconnectBridgeNetworkRequest**](DisconnectBridgeNetworkRequest.md) |  | 

### Return type

 (empty response body)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: application/json
- **Accept**: Not defined

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## GetContainerNetwork

> BridgeNetworkDetails GetContainerNetwork(ctx, idOrName).Execute()

Inspect a single bridge network by id or by name.



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
	idOrName := "idOrName_example" // string | Network id or name

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.ContainerNetworksAPI.GetContainerNetwork(context.Background(), idOrName).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `ContainerNetworksAPI.GetContainerNetwork``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `GetContainerNetwork`: BridgeNetworkDetails
	fmt.Fprintf(os.Stdout, "Response from `ContainerNetworksAPI.GetContainerNetwork`: %v\n", resp)
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**idOrName** | **string** | Network id or name | 

### Other Parameters

Other parameters are passed through a pointer to a apiGetContainerNetworkRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------


### Return type

[**BridgeNetworkDetails**](BridgeNetworkDetails.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## ListContainerNetworks

> []BridgeNetwork ListContainerNetworks(ctx).Label(label).Execute()

List all bridge networks, optionally filtered by label.



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
	label := "label_example" // string | Optional label filter in `key=value` form. Only networks whose labels contain a matching pair are returned. (optional)

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.ContainerNetworksAPI.ListContainerNetworks(context.Background()).Label(label).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `ContainerNetworksAPI.ListContainerNetworks``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `ListContainerNetworks`: []BridgeNetwork
	fmt.Fprintf(os.Stdout, "Response from `ContainerNetworksAPI.ListContainerNetworks`: %v\n", resp)
}
```

### Path Parameters



### Other Parameters

Other parameters are passed through a pointer to a apiListContainerNetworksRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
 **label** | **string** | Optional label filter in &#x60;key&#x3D;value&#x60; form. Only networks whose labels contain a matching pair are returned. | 

### Return type

[**[]BridgeNetwork**](BridgeNetwork.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)

