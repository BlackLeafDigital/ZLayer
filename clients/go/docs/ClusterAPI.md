# \ClusterAPI

All URIs are relative to *http://localhost*

Method | HTTP request | Description
------------- | ------------- | -------------
[**ClusterForceLeader**](ClusterAPI.md#ClusterForceLeader) | **Post** /api/v1/cluster/force-leader | Force this node to become the cluster leader (disaster recovery).
[**ClusterHeartbeat**](ClusterAPI.md#ClusterHeartbeat) | **Post** /api/v1/cluster/heartbeat | Handle node heartbeat.
[**ClusterJoin**](ClusterAPI.md#ClusterJoin) | **Post** /api/v1/cluster/join | Handle a cluster join request.
[**ClusterListGossipPeers**](ClusterAPI.md#ClusterListGossipPeers) | **Get** /api/v1/cluster/gossip/peers | &#x60;GET /api/v1/cluster/gossip/peers&#x60; — list peers known via the gossip pool.
[**ClusterListNodes**](ClusterAPI.md#ClusterListNodes) | **Get** /api/v1/cluster/nodes | List all nodes visible in the Raft cluster state.
[**ClusterListWorkers**](ClusterAPI.md#ClusterListWorkers) | **Get** /api/v1/cluster/workers | &#x60;GET /api/v1/cluster/workers&#x60; — list currently-leased worker-tier workers.
[**ClusterSetNodeLabels**](ClusterAPI.md#ClusterSetNodeLabels) | **Post** /api/v1/cluster/nodes/{id}/labels | Set or remove labels on a node, persisted in raft cluster state so the scheduler&#39;s &#x60;NodeSelector&#x60; placement honors them. &#x60;labels&#x60; entries are inserted/overwritten; &#x60;remove&#x60; keys are deleted.
[**ClusterUpgrade**](ClusterAPI.md#ClusterUpgrade) | **Post** /api/v1/cluster/upgrade | Drive a rolling daemon-binary upgrade across every follower.
[**ClusterUpgradeSelf**](ClusterAPI.md#ClusterUpgradeSelf) | **Post** /api/v1/cluster/upgrade-self | Trigger a daemon-binary self-upgrade on the local (leader) node.



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
	openapiclient "github.com/GIT_USER_ID/GIT_REPO_ID/zlayer"
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
	openapiclient "github.com/GIT_USER_ID/GIT_REPO_ID/zlayer"
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
	openapiclient "github.com/GIT_USER_ID/GIT_REPO_ID/zlayer"
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


## ClusterListGossipPeers

> []GossipPeerSummary ClusterListGossipPeers(ctx).Execute()

`GET /api/v1/cluster/gossip/peers` — list peers known via the gossip pool.



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
	resp, r, err := apiClient.ClusterAPI.ClusterListGossipPeers(context.Background()).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `ClusterAPI.ClusterListGossipPeers``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `ClusterListGossipPeers`: []GossipPeerSummary
	fmt.Fprintf(os.Stdout, "Response from `ClusterAPI.ClusterListGossipPeers`: %v\n", resp)
}
```

### Path Parameters

This endpoint does not need any parameter.

### Other Parameters

Other parameters are passed through a pointer to a apiClusterListGossipPeersRequest struct via the builder pattern


### Return type

[**[]GossipPeerSummary**](GossipPeerSummary.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
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
	openapiclient "github.com/GIT_USER_ID/GIT_REPO_ID/zlayer"
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


## ClusterListWorkers

> []WorkerSummary ClusterListWorkers(ctx).Execute()

`GET /api/v1/cluster/workers` — list currently-leased worker-tier workers.



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
	resp, r, err := apiClient.ClusterAPI.ClusterListWorkers(context.Background()).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `ClusterAPI.ClusterListWorkers``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `ClusterListWorkers`: []WorkerSummary
	fmt.Fprintf(os.Stdout, "Response from `ClusterAPI.ClusterListWorkers`: %v\n", resp)
}
```

### Path Parameters

This endpoint does not need any parameter.

### Other Parameters

Other parameters are passed through a pointer to a apiClusterListWorkersRequest struct via the builder pattern


### Return type

[**[]WorkerSummary**](WorkerSummary.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## ClusterSetNodeLabels

> UpdateLabelsResponse ClusterSetNodeLabels(ctx, id).UpdateLabelsRequest(updateLabelsRequest).Execute()

Set or remove labels on a node, persisted in raft cluster state so the scheduler's `NodeSelector` placement honors them. `labels` entries are inserted/overwritten; `remove` keys are deleted.



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
	id := int64(789) // int64 | Raft node id
	updateLabelsRequest := *openapiclient.NewUpdateLabelsRequest(map[string]string{"key": "Inner_example"}, []string{"Remove_example"}) // UpdateLabelsRequest | 

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.ClusterAPI.ClusterSetNodeLabels(context.Background(), id).UpdateLabelsRequest(updateLabelsRequest).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `ClusterAPI.ClusterSetNodeLabels``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `ClusterSetNodeLabels`: UpdateLabelsResponse
	fmt.Fprintf(os.Stdout, "Response from `ClusterAPI.ClusterSetNodeLabels`: %v\n", resp)
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**id** | **int64** | Raft node id | 

### Other Parameters

Other parameters are passed through a pointer to a apiClusterSetNodeLabelsRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------

 **updateLabelsRequest** | [**UpdateLabelsRequest**](UpdateLabelsRequest.md) |  | 

### Return type

[**UpdateLabelsResponse**](UpdateLabelsResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: application/json
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## ClusterUpgrade

> ClusterUpgradeResult ClusterUpgrade(ctx).ClusterUpgradeRequest(clusterUpgradeRequest).Execute()

Drive a rolling daemon-binary upgrade across every follower.



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
	clusterUpgradeRequest := *openapiclient.NewClusterUpgradeRequest() // ClusterUpgradeRequest | 

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.ClusterAPI.ClusterUpgrade(context.Background()).ClusterUpgradeRequest(clusterUpgradeRequest).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `ClusterAPI.ClusterUpgrade``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `ClusterUpgrade`: ClusterUpgradeResult
	fmt.Fprintf(os.Stdout, "Response from `ClusterAPI.ClusterUpgrade`: %v\n", resp)
}
```

### Path Parameters



### Other Parameters

Other parameters are passed through a pointer to a apiClusterUpgradeRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
 **clusterUpgradeRequest** | [**ClusterUpgradeRequest**](ClusterUpgradeRequest.md) |  | 

### Return type

[**ClusterUpgradeResult**](ClusterUpgradeResult.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: application/json
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## ClusterUpgradeSelf

> UpgradeStartResponse ClusterUpgradeSelf(ctx).ClusterUpgradeSelfRequest(clusterUpgradeSelfRequest).Execute()

Trigger a daemon-binary self-upgrade on the local (leader) node.



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
	clusterUpgradeSelfRequest := *openapiclient.NewClusterUpgradeSelfRequest() // ClusterUpgradeSelfRequest | 

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.ClusterAPI.ClusterUpgradeSelf(context.Background()).ClusterUpgradeSelfRequest(clusterUpgradeSelfRequest).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `ClusterAPI.ClusterUpgradeSelf``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `ClusterUpgradeSelf`: UpgradeStartResponse
	fmt.Fprintf(os.Stdout, "Response from `ClusterAPI.ClusterUpgradeSelf`: %v\n", resp)
}
```

### Path Parameters



### Other Parameters

Other parameters are passed through a pointer to a apiClusterUpgradeSelfRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
 **clusterUpgradeSelfRequest** | [**ClusterUpgradeSelfRequest**](ClusterUpgradeSelfRequest.md) |  | 

### Return type

[**UpgradeStartResponse**](UpgradeStartResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: application/json
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)

