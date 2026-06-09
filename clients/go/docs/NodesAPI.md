# \NodesAPI

All URIs are relative to *http://localhost*

Method | HTTP request | Description
------------- | ------------- | -------------
[**GenerateJoinToken**](NodesAPI.md#GenerateJoinToken) | **Post** /api/v1/nodes/join-token | Generate a join token for new nodes to join the cluster.
[**GetNode**](NodesAPI.md#GetNode) | **Get** /api/v1/nodes/{id} | Get detailed information about a specific node.
[**ListNodes**](NodesAPI.md#ListNodes) | **Get** /api/v1/nodes | List all nodes in the cluster.



## GenerateJoinToken

> JoinTokenResponse GenerateJoinToken(ctx).Execute()

Generate a join token for new nodes to join the cluster.



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

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.NodesAPI.GenerateJoinToken(context.Background()).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `NodesAPI.GenerateJoinToken``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `GenerateJoinToken`: JoinTokenResponse
	fmt.Fprintf(os.Stdout, "Response from `NodesAPI.GenerateJoinToken`: %v\n", resp)
}
```

### Path Parameters

This endpoint does not need any parameter.

### Other Parameters

Other parameters are passed through a pointer to a apiGenerateJoinTokenRequest struct via the builder pattern


### Return type

[**JoinTokenResponse**](JoinTokenResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## GetNode

> NodeDetails GetNode(ctx, id).Execute()

Get detailed information about a specific node.



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
	id := int64(789) // int64 | Node identifier

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.NodesAPI.GetNode(context.Background(), id).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `NodesAPI.GetNode``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `GetNode`: NodeDetails
	fmt.Fprintf(os.Stdout, "Response from `NodesAPI.GetNode`: %v\n", resp)
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**id** | **int64** | Node identifier | 

### Other Parameters

Other parameters are passed through a pointer to a apiGetNodeRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------


### Return type

[**NodeDetails**](NodeDetails.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## ListNodes

> []NodeSummary ListNodes(ctx).Execute()

List all nodes in the cluster.



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

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.NodesAPI.ListNodes(context.Background()).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `NodesAPI.ListNodes``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `ListNodes`: []NodeSummary
	fmt.Fprintf(os.Stdout, "Response from `NodesAPI.ListNodes`: %v\n", resp)
}
```

### Path Parameters

This endpoint does not need any parameter.

### Other Parameters

Other parameters are passed through a pointer to a apiListNodesRequest struct via the builder pattern


### Return type

[**[]NodeSummary**](NodeSummary.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)

