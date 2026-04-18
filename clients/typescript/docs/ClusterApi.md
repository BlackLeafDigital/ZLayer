# ClusterApi

All URIs are relative to *http://localhost*

| Method | HTTP request | Description |
|------------- | ------------- | -------------|
| [**clusterForceLeader**](ClusterApi.md#clusterforceleader) | **POST** /api/v1/cluster/force-leader | Force this node to become the cluster leader (disaster recovery). |
| [**clusterHeartbeat**](ClusterApi.md#clusterheartbeat) | **POST** /api/v1/cluster/heartbeat | Handle node heartbeat. |
| [**clusterJoin**](ClusterApi.md#clusterjoinoperation) | **POST** /api/v1/cluster/join | Handle a cluster join request. |
| [**clusterListNodes**](ClusterApi.md#clusterlistnodes) | **GET** /api/v1/cluster/nodes | List all nodes visible in the Raft cluster state. |



## clusterForceLeader

> ForceLeaderResponse clusterForceLeader(forceLeaderRequest)

Force this node to become the cluster leader (disaster recovery).

&#x60;POST /api/v1/cluster/force-leader&#x60;  DESTRUCTIVE operation for when the original leader is permanently lost. Saves cluster state, shuts down Raft, writes a recovery marker. The daemon must be restarted to complete recovery.  # Errors  Returns an error if the confirmation string is wrong, the Raft coordinator is unavailable, this node is already the leader, or the recovery state cannot be saved.

### Example

```ts
import {
  Configuration,
  ClusterApi,
} from '@zlayer/client';
import type { ClusterForceLeaderRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new ClusterApi(config);

  const body = {
    // ForceLeaderRequest
    forceLeaderRequest: ...,
  } satisfies ClusterForceLeaderRequest;

  try {
    const data = await api.clusterForceLeader(body);
    console.log(data);
  } catch (error) {
    console.error(error);
  }
}

// Run the test
example().catch(console.error);
```

### Parameters


| Name | Type | Description  | Notes |
|------------- | ------------- | ------------- | -------------|
| **forceLeaderRequest** | [ForceLeaderRequest](ForceLeaderRequest.md) |  | |

### Return type

[**ForceLeaderResponse**](ForceLeaderResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: `application/json`
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Force-leader initiated |  -  |
| **400** | Invalid confirmation or leader still reachable |  -  |
| **500** | Failed to save recovery state |  -  |
| **503** | Raft coordinator not available |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## clusterHeartbeat

> clusterHeartbeat(heartbeatRequest)

Handle node heartbeat.

&#x60;POST /api/v1/cluster/heartbeat&#x60;  Accepts resource usage data from worker nodes and proposes an &#x60;UpdateNodeHeartbeat&#x60; to the Raft state machine.  # Panics  Panics if the system clock is before the Unix epoch.

### Example

```ts
import {
  Configuration,
  ClusterApi,
} from '@zlayer/client';
import type { ClusterHeartbeatRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const api = new ClusterApi();

  const body = {
    // HeartbeatRequest
    heartbeatRequest: ...,
  } satisfies ClusterHeartbeatRequest;

  try {
    const data = await api.clusterHeartbeat(body);
    console.log(data);
  } catch (error) {
    console.error(error);
  }
}

// Run the test
example().catch(console.error);
```

### Parameters


| Name | Type | Description  | Notes |
|------------- | ------------- | ------------- | -------------|
| **heartbeatRequest** | [HeartbeatRequest](HeartbeatRequest.md) |  | |

### Return type

`void` (Empty response body)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: `application/json`
- **Accept**: Not defined


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Heartbeat accepted |  -  |
| **500** | Failed to propose heartbeat |  -  |
| **503** | Raft coordinator not available |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## clusterJoin

> ClusterJoinResponse clusterJoin(clusterJoinRequest)

Handle a cluster join request.

&#x60;POST /api/v1/cluster/join&#x60;  Validates the join token, assigns a Raft node ID, calls &#x60;raft.add_member()&#x60;, and returns the assignment + peer list.  # Errors  Returns an error if the Raft coordinator is unavailable, the join token is invalid, IP allocation fails, or the Raft membership change fails.

### Example

```ts
import {
  Configuration,
  ClusterApi,
} from '@zlayer/client';
import type { ClusterJoinOperationRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const api = new ClusterApi();

  const body = {
    // ClusterJoinRequest
    clusterJoinRequest: ...,
  } satisfies ClusterJoinOperationRequest;

  try {
    const data = await api.clusterJoin(body);
    console.log(data);
  } catch (error) {
    console.error(error);
  }
}

// Run the test
example().catch(console.error);
```

### Parameters


| Name | Type | Description  | Notes |
|------------- | ------------- | ------------- | -------------|
| **clusterJoinRequest** | [ClusterJoinRequest](ClusterJoinRequest.md) |  | |

### Return type

[**ClusterJoinResponse**](ClusterJoinResponse.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: `application/json`
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Node joined successfully |  -  |
| **401** | Invalid join token |  -  |
| **500** | Failed to add member to Raft |  -  |
| **503** | Raft coordinator not available |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## clusterListNodes

> Array&lt;ClusterNodeSummary&gt; clusterListNodes()

List all nodes visible in the Raft cluster state.

&#x60;GET /api/v1/cluster/nodes&#x60;  # Errors  Returns an error if the cluster state cannot be read.

### Example

```ts
import {
  Configuration,
  ClusterApi,
} from '@zlayer/client';
import type { ClusterListNodesRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new ClusterApi(config);

  try {
    const data = await api.clusterListNodes();
    console.log(data);
  } catch (error) {
    console.error(error);
  }
}

// Run the test
example().catch(console.error);
```

### Parameters

This endpoint does not need any parameter.

### Return type

[**Array&lt;ClusterNodeSummary&gt;**](ClusterNodeSummary.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | List of cluster nodes |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)

