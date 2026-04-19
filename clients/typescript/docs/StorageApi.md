# StorageApi

All URIs are relative to *http://localhost*

| Method | HTTP request | Description |
|------------- | ------------- | -------------|
| [**getStorageStatus**](StorageApi.md#getstoragestatus) | **GET** /api/v1/storage/status | Get storage replication status. |



## getStorageStatus

> StorageStatusResponse getStorageStatus()

Get storage replication status.

Returns the current state of SQLite-to-S3 replication including whether it is enabled, the last sync time, and pending change count.  # Errors  Returns an error if authentication fails.

### Example

```ts
import {
  Configuration,
  StorageApi,
} from '@zlayer/client';
import type { GetStorageStatusRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new StorageApi(config);

  try {
    const data = await api.getStorageStatus();
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

[**StorageStatusResponse**](StorageStatusResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Storage replication status |  -  |
| **401** | Unauthorized |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)

