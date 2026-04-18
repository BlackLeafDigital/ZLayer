# VolumesApi

All URIs are relative to *http://localhost*

| Method | HTTP request | Description |
|------------- | ------------- | -------------|
| [**deleteVolume**](VolumesApi.md#deletevolume) | **DELETE** /api/v1/volumes/{name} | Delete a volume by name. |
| [**listVolumes**](VolumesApi.md#listvolumes) | **GET** /api/v1/volumes | List all volumes on disk. |



## deleteVolume

> deleteVolume(name, force)

Delete a volume by name.

Removes a volume directory from disk. By default, only empty volumes can be deleted. Use &#x60;?force&#x3D;true&#x60; to remove non-empty volumes.  # Errors  Returns an error if the volume is not found or cannot be removed.

### Example

```ts
import {
  Configuration,
  VolumesApi,
} from '@zlayer/client';
import type { DeleteVolumeRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new VolumesApi(config);

  const body = {
    // string | Volume name
    name: name_example,
    // boolean | Force removal of non-empty volumes
    force: true,
  } satisfies DeleteVolumeRequest;

  try {
    const data = await api.deleteVolume(body);
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
| **name** | `string` | Volume name | [Defaults to `undefined`] |
| **force** | `boolean` | Force removal of non-empty volumes | [Defaults to `undefined`] |

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
| **204** | Volume deleted |  -  |
| **401** | Unauthorized |  -  |
| **403** | Forbidden |  -  |
| **404** | Volume not found |  -  |
| **409** | Volume is non-empty (use ?force&#x3D;true) |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## listVolumes

> Array&lt;VolumeSummary&gt; listVolumes()

List all volumes on disk.

Enumerates subdirectories under the volume base directory and returns metadata for each one.  # Errors  Returns an error if the volume directory cannot be read.

### Example

```ts
import {
  Configuration,
  VolumesApi,
} from '@zlayer/client';
import type { ListVolumesRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new VolumesApi(config);

  try {
    const data = await api.listVolumes();
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

[**Array&lt;VolumeSummary&gt;**](VolumeSummary.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | List of volumes |  -  |
| **401** | Unauthorized |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)

