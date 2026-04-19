# SyncsApi

All URIs are relative to *http://localhost*

| Method | HTTP request | Description |
|------------- | ------------- | -------------|
| [**applySync**](SyncsApi.md#applysync) | **POST** /api/v1/syncs/{id}/apply | Apply a sync — real reconcile against the API. |
| [**createSync**](SyncsApi.md#createsyncoperation) | **POST** /api/v1/syncs | Create a new sync. |
| [**deleteSync**](SyncsApi.md#deletesync) | **DELETE** /api/v1/syncs/{id} | Delete a sync. |
| [**diffSync**](SyncsApi.md#diffsync) | **GET** /api/v1/syncs/{id}/diff | Compute a diff for a sync (scan git path vs. remote resources). |
| [**listSyncs**](SyncsApi.md#listsyncs) | **GET** /api/v1/syncs | List all syncs. |



## applySync

> SyncApplyResponse applySync(id)

Apply a sync — real reconcile against the API.

For each resource in the manifest directory: - &#x60;deployment&#x60; YAMLs are parsed, validated, and upserted into the   deployment store. - &#x60;job&#x60; / &#x60;cron&#x60; kinds are recorded as skipped with an explanatory message   (standalone job/cron storage does not yet exist; schedule services via   &#x60;DeploymentSpec&#x60; instead). - Unknown kinds are skipped with &#x60;\&quot;unknown kind: {kind}\&quot;&#x60;.  If &#x60;sync.delete_missing&#x60; is &#x60;true&#x60;, deployments present in the store but absent from the manifest directory are deleted. Otherwise deletions are recorded as skipped with reason &#x60;\&quot;delete_missing&#x3D;false\&quot;&#x60;.  On success the sync\&#39;s &#x60;last_applied_sha&#x60; is refreshed from the current checkout and persisted.  # Errors  Returns [&#x60;ApiError::NotFound&#x60;] if the sync id does not exist, or [&#x60;ApiError::Internal&#x60;] if scanning fails.

### Example

```ts
import {
  Configuration,
  SyncsApi,
} from '@zlayer/client';
import type { ApplySyncRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new SyncsApi(config);

  const body = {
    // string | Sync id
    id: id_example,
  } satisfies ApplySyncRequest;

  try {
    const data = await api.applySync(body);
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
| **id** | `string` | Sync id | [Defaults to `undefined`] |

### Return type

[**SyncApplyResponse**](SyncApplyResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Apply result |  -  |
| **404** | Sync not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## createSync

> StoredSync createSync(createSyncRequest)

Create a new sync.

# Errors  Returns [&#x60;ApiError::Validation&#x60;] if required fields are missing, or [&#x60;ApiError::Conflict&#x60;] if a sync with the same name already exists.

### Example

```ts
import {
  Configuration,
  SyncsApi,
} from '@zlayer/client';
import type { CreateSyncOperationRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new SyncsApi(config);

  const body = {
    // CreateSyncRequest
    createSyncRequest: ...,
  } satisfies CreateSyncOperationRequest;

  try {
    const data = await api.createSync(body);
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
| **createSyncRequest** | [CreateSyncRequest](CreateSyncRequest.md) |  | |

### Return type

[**StoredSync**](StoredSync.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: `application/json`
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **201** | Sync created |  -  |
| **400** | Validation error |  -  |
| **409** | Sync name already exists |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## deleteSync

> deleteSync(id)

Delete a sync.

# Errors  Returns [&#x60;ApiError::NotFound&#x60;] if the sync id does not exist, or [&#x60;ApiError::Internal&#x60;] if the store operation fails.

### Example

```ts
import {
  Configuration,
  SyncsApi,
} from '@zlayer/client';
import type { DeleteSyncRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new SyncsApi(config);

  const body = {
    // string | Sync id
    id: id_example,
  } satisfies DeleteSyncRequest;

  try {
    const data = await api.deleteSync(body);
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
| **id** | `string` | Sync id | [Defaults to `undefined`] |

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
| **204** | Sync deleted |  -  |
| **404** | Sync not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## diffSync

> SyncDiffResponse diffSync(id)

Compute a diff for a sync (scan git path vs. remote resources).

# Errors  Returns [&#x60;ApiError::NotFound&#x60;] if the sync id does not exist, or [&#x60;ApiError::Internal&#x60;] if scanning fails.

### Example

```ts
import {
  Configuration,
  SyncsApi,
} from '@zlayer/client';
import type { DiffSyncRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new SyncsApi(config);

  const body = {
    // string | Sync id
    id: id_example,
  } satisfies DiffSyncRequest;

  try {
    const data = await api.diffSync(body);
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
| **id** | `string` | Sync id | [Defaults to `undefined`] |

### Return type

[**SyncDiffResponse**](SyncDiffResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Computed diff |  -  |
| **404** | Sync not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## listSyncs

> Array&lt;StoredSync&gt; listSyncs()

List all syncs.

# Errors  Returns [&#x60;ApiError::Internal&#x60;] if the sync store fails.

### Example

```ts
import {
  Configuration,
  SyncsApi,
} from '@zlayer/client';
import type { ListSyncsRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new SyncsApi(config);

  try {
    const data = await api.listSyncs();
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

[**Array&lt;StoredSync&gt;**](StoredSync.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | List of syncs |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)

