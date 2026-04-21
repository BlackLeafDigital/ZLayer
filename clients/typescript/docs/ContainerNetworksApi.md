# ContainerNetworksApi

All URIs are relative to *http://localhost*

| Method | HTTP request | Description |
|------------- | ------------- | -------------|
| [**connectContainerNetwork**](ContainerNetworksApi.md#connectcontainernetwork) | **POST** /api/v1/container-networks/{id_or_name}/connect | Attach a container to a network. |
| [**createContainerNetwork**](ContainerNetworksApi.md#createcontainernetwork) | **POST** /api/v1/container-networks | Create a new bridge or overlay network. |
| [**deleteContainerNetwork**](ContainerNetworksApi.md#deletecontainernetwork) | **DELETE** /api/v1/container-networks/{id_or_name} | Delete a bridge network. Refuses if the network still has attachments unless &#x60;?force&#x3D;true&#x60; is set. |
| [**disconnectContainerNetwork**](ContainerNetworksApi.md#disconnectcontainernetwork) | **POST** /api/v1/container-networks/{id_or_name}/disconnect | Detach a container from a network. |
| [**getContainerNetwork**](ContainerNetworksApi.md#getcontainernetwork) | **GET** /api/v1/container-networks/{id_or_name} | Inspect a single bridge network by id or by name. |
| [**listContainerNetworks**](ContainerNetworksApi.md#listcontainernetworks) | **GET** /api/v1/container-networks | List all bridge networks, optionally filtered by label. |



## connectContainerNetwork

> connectContainerNetwork(idOrName, connectBridgeNetworkRequest)

Attach a container to a network.

# Errors  - [&#x60;ApiError::BadRequest&#x60;] for an empty &#x60;container_id&#x60; or invalid   &#x60;ipv4_address&#x60;. - [&#x60;ApiError::NotFound&#x60;] when no network matches &#x60;id_or_name&#x60;.

### Example

```ts
import {
  Configuration,
  ContainerNetworksApi,
} from '@zlayer/api-client';
import type { ConnectContainerNetworkRequest } from '@zlayer/api-client';

async function example() {
  console.log("🚀 Testing @zlayer/api-client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new ContainerNetworksApi(config);

  const body = {
    // string | Network id or name
    idOrName: idOrName_example,
    // ConnectBridgeNetworkRequest
    connectBridgeNetworkRequest: ...,
  } satisfies ConnectContainerNetworkRequest;

  try {
    const data = await api.connectContainerNetwork(body);
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
| **idOrName** | `string` | Network id or name | [Defaults to `undefined`] |
| **connectBridgeNetworkRequest** | [ConnectBridgeNetworkRequest](ConnectBridgeNetworkRequest.md) |  | |

### Return type

`void` (Empty response body)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: `application/json`
- **Accept**: Not defined


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **204** | Container connected |  -  |
| **400** | Invalid request (e.g. bad ipv4_address) |  -  |
| **401** | Unauthorized |  -  |
| **404** | Network not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## createContainerNetwork

> BridgeNetwork createContainerNetwork(createBridgeNetworkRequest)

Create a new bridge or overlay network.

# Errors  - [&#x60;ApiError::BadRequest&#x60;] if the network name or subnet is malformed. - [&#x60;ApiError::Conflict&#x60;] if a network with the same name already exists. - [&#x60;ApiError::Forbidden&#x60;] / [&#x60;ApiError::Unauthorized&#x60;] from the auth layer.

### Example

```ts
import {
  Configuration,
  ContainerNetworksApi,
} from '@zlayer/api-client';
import type { CreateContainerNetworkRequest } from '@zlayer/api-client';

async function example() {
  console.log("🚀 Testing @zlayer/api-client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new ContainerNetworksApi(config);

  const body = {
    // CreateBridgeNetworkRequest
    createBridgeNetworkRequest: ...,
  } satisfies CreateContainerNetworkRequest;

  try {
    const data = await api.createContainerNetwork(body);
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
| **createBridgeNetworkRequest** | [CreateBridgeNetworkRequest](CreateBridgeNetworkRequest.md) |  | |

### Return type

[**BridgeNetwork**](BridgeNetwork.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: `application/json`
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **201** | Network created |  -  |
| **400** | Invalid request |  -  |
| **401** | Unauthorized |  -  |
| **403** | Forbidden |  -  |
| **409** | Network with that name already exists |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## deleteContainerNetwork

> deleteContainerNetwork(idOrName, force)

Delete a bridge network. Refuses if the network still has attachments unless &#x60;?force&#x3D;true&#x60; is set.

# Errors  - [&#x60;ApiError::NotFound&#x60;] when no network matches &#x60;id_or_name&#x60;. - [&#x60;ApiError::Conflict&#x60;] when the network still has attachments and   &#x60;force&#x60; is false. - Auth errors propagated from the &#x60;operator&#x60; role check.

### Example

```ts
import {
  Configuration,
  ContainerNetworksApi,
} from '@zlayer/api-client';
import type { DeleteContainerNetworkRequest } from '@zlayer/api-client';

async function example() {
  console.log("🚀 Testing @zlayer/api-client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new ContainerNetworksApi(config);

  const body = {
    // string | Network id or name
    idOrName: idOrName_example,
    // boolean | If true, delete even if the network still has attachments. (optional)
    force: true,
  } satisfies DeleteContainerNetworkRequest;

  try {
    const data = await api.deleteContainerNetwork(body);
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
| **idOrName** | `string` | Network id or name | [Defaults to `undefined`] |
| **force** | `boolean` | If true, delete even if the network still has attachments. | [Optional] [Defaults to `undefined`] |

### Return type

`void` (Empty response body)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: Not defined


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **204** | Network deleted |  -  |
| **401** | Unauthorized |  -  |
| **403** | Forbidden |  -  |
| **404** | Network not found |  -  |
| **409** | Network still has attachments (use ?force&#x3D;true) |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## disconnectContainerNetwork

> disconnectContainerNetwork(idOrName, disconnectBridgeNetworkRequest)

Detach a container from a network.

# Errors  - [&#x60;ApiError::BadRequest&#x60;] when &#x60;container_id&#x60; is empty. - [&#x60;ApiError::NotFound&#x60;] when the network or the container-on-network   pair is not known (unless &#x60;force&#x60; was passed).

### Example

```ts
import {
  Configuration,
  ContainerNetworksApi,
} from '@zlayer/api-client';
import type { DisconnectContainerNetworkRequest } from '@zlayer/api-client';

async function example() {
  console.log("🚀 Testing @zlayer/api-client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new ContainerNetworksApi(config);

  const body = {
    // string | Network id or name
    idOrName: idOrName_example,
    // DisconnectBridgeNetworkRequest
    disconnectBridgeNetworkRequest: ...,
  } satisfies DisconnectContainerNetworkRequest;

  try {
    const data = await api.disconnectContainerNetwork(body);
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
| **idOrName** | `string` | Network id or name | [Defaults to `undefined`] |
| **disconnectBridgeNetworkRequest** | [DisconnectBridgeNetworkRequest](DisconnectBridgeNetworkRequest.md) |  | |

### Return type

`void` (Empty response body)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: `application/json`
- **Accept**: Not defined


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **204** | Container disconnected |  -  |
| **401** | Unauthorized |  -  |
| **404** | Network or container not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## getContainerNetwork

> BridgeNetworkDetails getContainerNetwork(idOrName)

Inspect a single bridge network by id or by name.

# Errors  [&#x60;ApiError::NotFound&#x60;] when no network matches &#x60;id_or_name&#x60;.

### Example

```ts
import {
  Configuration,
  ContainerNetworksApi,
} from '@zlayer/api-client';
import type { GetContainerNetworkRequest } from '@zlayer/api-client';

async function example() {
  console.log("🚀 Testing @zlayer/api-client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new ContainerNetworksApi(config);

  const body = {
    // string | Network id or name
    idOrName: idOrName_example,
  } satisfies GetContainerNetworkRequest;

  try {
    const data = await api.getContainerNetwork(body);
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
| **idOrName** | `string` | Network id or name | [Defaults to `undefined`] |

### Return type

[**BridgeNetworkDetails**](BridgeNetworkDetails.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Network details |  -  |
| **401** | Unauthorized |  -  |
| **404** | Network not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## listContainerNetworks

> Array&lt;BridgeNetwork&gt; listContainerNetworks(label)

List all bridge networks, optionally filtered by label.

# Errors  [&#x60;ApiError::BadRequest&#x60;] when the &#x60;label&#x60; query param is not in &#x60;key&#x3D;value&#x60; form.

### Example

```ts
import {
  Configuration,
  ContainerNetworksApi,
} from '@zlayer/api-client';
import type { ListContainerNetworksRequest } from '@zlayer/api-client';

async function example() {
  console.log("🚀 Testing @zlayer/api-client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new ContainerNetworksApi(config);

  const body = {
    // string | Optional label filter in `key=value` form. Only networks whose labels contain a matching pair are returned. (optional)
    label: label_example,
  } satisfies ListContainerNetworksRequest;

  try {
    const data = await api.listContainerNetworks(body);
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
| **label** | `string` | Optional label filter in &#x60;key&#x3D;value&#x60; form. Only networks whose labels contain a matching pair are returned. | [Optional] [Defaults to `undefined`] |

### Return type

[**Array&lt;BridgeNetwork&gt;**](BridgeNetwork.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | List of bridge networks |  -  |
| **400** | Invalid label filter |  -  |
| **401** | Unauthorized |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)

