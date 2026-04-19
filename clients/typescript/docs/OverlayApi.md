# OverlayApi

All URIs are relative to *http://localhost*

| Method | HTTP request | Description |
|------------- | ------------- | -------------|
| [**getDnsStatus**](OverlayApi.md#getdnsstatus) | **GET** /api/v1/overlay/dns | Get DNS service status. |
| [**getIpAllocation**](OverlayApi.md#getipallocation) | **GET** /api/v1/overlay/ip-alloc | Get IP allocation status. |
| [**getOverlayPeers**](OverlayApi.md#getoverlaypeers) | **GET** /api/v1/overlay/peers | Get list of overlay peers. |
| [**getOverlayStatus**](OverlayApi.md#getoverlaystatus) | **GET** /api/v1/overlay/status | Get overlay network status. |



## getDnsStatus

> DnsStatusResponse getDnsStatus()

Get DNS service status.

Returns DNS server configuration and registered service count. When DNS is not configured, returns a disabled status (not an error).

### Example

```ts
import {
  Configuration,
  OverlayApi,
} from '@zlayer/client';
import type { GetDnsStatusRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const api = new OverlayApi();

  try {
    const data = await api.getDnsStatus();
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

[**DnsStatusResponse**](DnsStatusResponse.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | DNS service status |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## getIpAllocation

> IpAllocationResponse getIpAllocation()

Get IP allocation status.

Returns the overlay network\&#39;s IP allocation statistics including CIDR, total IPs, allocated count, and utilization. Available on any node that has an active overlay (not just leaders).  # Errors  Returns an error if the overlay is not initialized.

### Example

```ts
import {
  Configuration,
  OverlayApi,
} from '@zlayer/client';
import type { GetIpAllocationRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const api = new OverlayApi();

  try {
    const data = await api.getIpAllocation();
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

[**IpAllocationResponse**](IpAllocationResponse.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | IP allocation status |  -  |
| **503** | Not a leader node or overlay not initialized |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## getOverlayPeers

> PeerListResponse getOverlayPeers()

Get list of overlay peers.

Returns peer information from the overlay transport\&#39;s UAPI state. When the overlay is not initialized, returns 503.  # Errors  Returns an error if the overlay network is not initialized.

### Example

```ts
import {
  Configuration,
  OverlayApi,
} from '@zlayer/client';
import type { GetOverlayPeersRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const api = new OverlayApi();

  try {
    const data = await api.getOverlayPeers();
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

[**PeerListResponse**](PeerListResponse.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | List of overlay peers |  -  |
| **503** | Overlay not initialized |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## getOverlayStatus

> OverlayStatusResponse getOverlayStatus()

Get overlay network status.

Returns the current overlay network status including interface name, node IP, CIDR, peer counts, and whether the transport is active. Returns 503 if the overlay network is not initialized (e.g. host networking mode).  # Errors  Returns an error if the overlay network is not initialized.

### Example

```ts
import {
  Configuration,
  OverlayApi,
} from '@zlayer/client';
import type { GetOverlayStatusRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const api = new OverlayApi();

  try {
    const data = await api.getOverlayStatus();
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

[**OverlayStatusResponse**](OverlayStatusResponse.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Overlay network status |  -  |
| **503** | Overlay not initialized |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)

