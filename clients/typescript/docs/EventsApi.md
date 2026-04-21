# EventsApi

All URIs are relative to *http://localhost*

| Method | HTTP request | Description |
|------------- | ------------- | -------------|
| [**streamEvents**](EventsApi.md#streamevents) | **GET** /api/v1/events | Stream container lifecycle events as Server-Sent Events. |



## streamEvents

> streamEvents(follow, label)

Stream container lifecycle events as Server-Sent Events.

# Errors  Returns &#x60;400&#x60; if a &#x60;label&#x60; query parameter is malformed, &#x60;401&#x60; if the caller is not authenticated.

### Example

```ts
import {
  Configuration,
  EventsApi,
} from '@zlayer/api-client';
import type { StreamEventsRequest } from '@zlayer/api-client';

async function example() {
  console.log("🚀 Testing @zlayer/api-client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new EventsApi(config);

  const body = {
    // boolean | Follow the event stream. Default: `true`. Reserved for parity with Docker-compat tooling; this endpoint is always streaming. (optional)
    follow: true,
    // Array<string> | Label filter in `k=v` form. Repeatable. An event passes only if all filters match (AND semantics). (optional)
    label: ...,
  } satisfies StreamEventsRequest;

  try {
    const data = await api.streamEvents(body);
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
| **follow** | `boolean` | Follow the event stream. Default: &#x60;true&#x60;. Reserved for parity with Docker-compat tooling; this endpoint is always streaming. | [Optional] [Defaults to `undefined`] |
| **label** | `Array<string>` | Label filter in &#x60;k&#x3D;v&#x60; form. Repeatable. An event passes only if all filters match (AND semantics). | [Optional] |

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
| **200** | SSE stream of container lifecycle events |  -  |
| **400** | Invalid label filter |  -  |
| **401** | Unauthorized |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)

