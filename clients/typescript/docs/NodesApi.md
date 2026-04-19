# NodesApi

All URIs are relative to *http://localhost*

| Method | HTTP request | Description |
|------------- | ------------- | -------------|
| [**generateJoinToken**](NodesApi.md#generatejointoken) | **POST** /api/v1/nodes/join-token | Generate a join token for new nodes to join the cluster. |
| [**getNode**](NodesApi.md#getnode) | **GET** /api/v1/nodes/{id} | Get detailed information about a specific node. |
| [**listNodes**](NodesApi.md#listnodes) | **GET** /api/v1/nodes | List all nodes in the cluster. |
| [**updateNodeLabels**](NodesApi.md#updatenodelabels) | **POST** /api/v1/nodes/{id}/labels | Update labels on a node. |



## generateJoinToken

> JoinTokenResponse generateJoinToken()

Generate a join token for new nodes to join the cluster.

# Errors  Returns an error if cluster management is not available or the user lacks permission.

### Example

```ts
import {
  Configuration,
  NodesApi,
} from '@zlayer/client';
import type { GenerateJoinTokenRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new NodesApi(config);

  try {
    const data = await api.generateJoinToken();
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

[**JoinTokenResponse**](JoinTokenResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Join token generated |  -  |
| **401** | Unauthorized |  -  |
| **403** | Forbidden - admin role required |  -  |
| **503** | Service unavailable - cluster not ready |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## getNode

> NodeDetails getNode(id)

Get detailed information about a specific node.

# Errors  Returns an error if the node is not found.

### Example

```ts
import {
  Configuration,
  NodesApi,
} from '@zlayer/client';
import type { GetNodeRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new NodesApi(config);

  const body = {
    // number | Node identifier
    id: 789,
  } satisfies GetNodeRequest;

  try {
    const data = await api.getNode(body);
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
| **id** | `number` | Node identifier | [Defaults to `undefined`] |

### Return type

[**NodeDetails**](NodeDetails.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Node details |  -  |
| **401** | Unauthorized |  -  |
| **404** | Node not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## listNodes

> Array&lt;NodeSummary&gt; listNodes()

List all nodes in the cluster.

# Errors  Returns an error if authentication fails.

### Example

```ts
import {
  Configuration,
  NodesApi,
} from '@zlayer/client';
import type { ListNodesRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new NodesApi(config);

  try {
    const data = await api.listNodes();
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

[**Array&lt;NodeSummary&gt;**](NodeSummary.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | List of cluster nodes |  -  |
| **401** | Unauthorized |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## updateNodeLabels

> UpdateLabelsResponse updateNodeLabels(id, updateLabelsRequest)

Update labels on a node.

# Errors  Returns an error if the node is not found or the user lacks permission.

### Example

```ts
import {
  Configuration,
  NodesApi,
} from '@zlayer/client';
import type { UpdateNodeLabelsRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new NodesApi(config);

  const body = {
    // number | Node identifier
    id: 789,
    // UpdateLabelsRequest
    updateLabelsRequest: ...,
  } satisfies UpdateNodeLabelsRequest;

  try {
    const data = await api.updateNodeLabels(body);
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
| **id** | `number` | Node identifier | [Defaults to `undefined`] |
| **updateLabelsRequest** | [UpdateLabelsRequest](UpdateLabelsRequest.md) |  | |

### Return type

[**UpdateLabelsResponse**](UpdateLabelsResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: `application/json`
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Labels updated |  -  |
| **401** | Unauthorized |  -  |
| **403** | Forbidden - admin role required |  -  |
| **404** | Node not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)

