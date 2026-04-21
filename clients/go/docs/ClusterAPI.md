# \ClusterAPI

All URIs are relative to *http://localhost*

Method | HTTP request | Description
------------- | ------------- | -------------
[**ClusterForceLeader**](ClusterAPI.md#ClusterForceLeader) | **Post** /api/v1/cluster/force-leader | Force this node to become the cluster leader (disaster recovery).
[**ClusterHeartbeat**](ClusterAPI.md#ClusterHeartbeat) | **Post** /api/v1/cluster/heartbeat | Handle node heartbeat.
[**ClusterJoin**](ClusterAPI.md#ClusterJoin) | **Post** /api/v1/cluster/join | Handle a cluster join request.
[**ClusterListNodes**](ClusterAPI.md#ClusterListNodes) | **Get** /api/v1/cluster/nodes | List all nodes visible in the Raft cluster state.



## ClusterForceLeader

> ForceLeaderResponse ClusterForceLeader(ctx).ForceLeaderRequest(forceLeaderRequest).Execute()

Force this node to become the cluster leader (disaster recovery).



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
	forceLeaderRequest := *openapiclient.NewForceLeaderRequest("Confirm_example") // ForceLeaderRequest | 

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.ClusterAPI.ClusterForceLeader(context.Background()).ForceLeaderRequest(forceLeaderRequest).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `ClusterAPI.ClusterForceLeader``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `ClusterForceLeader`: ForceLeaderResponse
	fmt.Fprintf(os.Stdout, "Response from `ClusterAPI.ClusterForceLeader`: %v\n", resp)
}
```

### Path Parameters



### Other Parameters

Other parameters are passed through a pointer to a apiClusterForceLeaderRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
 **forceLeaderRequest** | [**ForceLeaderRequest**](ForceLeaderRequest.md) |  | 

### Return type

[**ForceLeaderResponse**](ForceLeaderResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: application/json
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## ClusterHeartbeat

> ClusterHeartbeat(ctx).HeartbeatRequest(heartbeatRequest).Execute()

Handle node heartbeat.



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
	heartbeatRequest := *openapiclient.NewHeartbeatRequest(float64(123), int64(123), int64(123), int64(123)) // HeartbeatRequest | 

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	r, err := apiClient.ClusterAPI.ClusterHeartbeat(context.Background()).HeartbeatRequest(heartbeatRequest).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `ClusterAPI.ClusterHeartbeat``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
}
```

### Path Parameters



### Other Parameters

Other parameters are passed through a pointer to a apiClusterHeartbeatRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
 **heartbeatRequest** | [**HeartbeatRequest**](HeartbeatRequest.md) |  | 

### Return type

 (empty response body)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: application/json
- **Accept**: Not defined

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## ClusterJoin

> ClusterJoinResponse ClusterJoin(ctx).ClusterJoinRequest(clusterJoinRequest).Execute()

Handle a cluster join request.



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
	clusterJoinRequest := *openapiclient.NewClusterJoinRequest("AdvertiseAddr_example", int32(123), int32(123), "Token_example", "WgPublicKey_example") // ClusterJoinRequest | 

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.ClusterAPI.ClusterJoin(context.Background()).ClusterJoinRequest(clusterJoinRequest).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `ClusterAPI.ClusterJoin``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `ClusterJoin`: ClusterJoinResponse
	fmt.Fprintf(os.Stdout, "Response from `ClusterAPI.ClusterJoin`: %v\n", resp)
}
```

### Path Parameters



### Other Parameters

Other parameters are passed through a pointer to a apiClusterJoinRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
 **clusterJoinRequest** | [**ClusterJoinRequest**](ClusterJoinRequest.md) |  | 

### Return type

[**ClusterJoinResponse**](ClusterJoinResponse.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: application/json
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## ClusterListNodes

> []ClusterNodeSummary ClusterListNodes(ctx).Execute()

List all nodes visible in the Raft cluster state.



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
	resp, r, err := apiClient.ClusterAPI.ClusterListNodes(context.Background()).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `ClusterAPI.ClusterListNodes``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `ClusterListNodes`: []ClusterNodeSummary
	fmt.Fprintf(os.Stdout, "Response from `ClusterAPI.ClusterListNodes`: %v\n", resp)
}
```

### Path Parameters

This endpoint does not need any parameter.

### Other Parameters

Other parameters are passed through a pointer to a apiClusterListNodesRequest struct via the builder pattern


### Return type

[**[]ClusterNodeSummary**](ClusterNodeSummary.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)

