# VolumesApi

All URIs are relative to *http://localhost*

| Method | HTTP request | Description |
|------------- | ------------- | -------------|
| [**createVolume**](VolumesApi.md#createvolumeoperation) | **POST** /api/v1/volumes | Create a new named volume. |
| [**deleteVolume**](VolumesApi.md#deletevolume) | **DELETE** /api/v1/volumes/{name} | Delete a volume by name. |
| [**getVolume**](VolumesApi.md#getvolume) | **GET** /api/v1/volumes/{name} | Inspect a single volume by name. |
| [**listVolumes**](VolumesApi.md#listvolumes) | **GET** /api/v1/volumes | List all volumes on disk. |



## createVolume

> VolumeInfo createVolume(createVolumeRequest)

Create a new named volume.

Creates &#x60;state.volume_dir/{name}&#x60; on disk and writes a &#x60;.metadata.json&#x60; sidecar capturing labels, size, tier, and the creation timestamp.  # Errors  - 400 Bad Request — invalid name, size, or tier. - 403 Forbidden — caller lacks the &#x60;operator&#x60; role. - 409 Conflict — a volume with this name already exists. - 500 Internal — filesystem errors.

### Example

```ts
import {
  Configuration,
  VolumesApi,
} from '@zlayer/api-client';
import type { CreateVolumeOperationRequest } from '@zlayer/api-client';

async function example() {
  console.log("🚀 Testing @zlayer/api-client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new VolumesApi(config);

  const body = {
    // CreateVolumeRequest
    createVolumeRequest: ...,
  } satisfies CreateVolumeOperationRequest;

  try {
    const data = await api.createVolume(body);
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
| **createVolumeRequest** | [CreateVolumeRequest](CreateVolumeRequest.md) |  | |

### Return type

[**VolumeInfo**](VolumeInfo.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: `application/json`
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **201** | Volume created |  -  |
| **400** | Bad request |  -  |
| **401** | Unauthorized |  -  |
| **403** | Forbidden |  -  |
| **409** | Volume already exists |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## deleteVolume

> deleteVolume(name, force)

Delete a volume by name.

By default, refuses to remove a volume that is non-empty OR that any container currently reports mounting. &#x60;?force&#x3D;true&#x60; overrides both checks.  # Errors  - 404 Not Found — no such volume. - 409 Conflict — non-empty or in-use without &#x60;?force&#x3D;true&#x60;. - 403 Forbidden — caller lacks the &#x60;operator&#x60; role.

### Example

```ts
import {
  Configuration,
  VolumesApi,
} from '@zlayer/api-client';
import type { DeleteVolumeRequest } from '@zlayer/api-client';

async function example() {
  console.log("🚀 Testing @zlayer/api-client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new VolumesApi(config);

  const body = {
    // string | Volume name
    name: name_example,
    // boolean | Force removal of non-empty or in-use volumes
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
| **force** | `boolean` | Force removal of non-empty or in-use volumes | [Defaults to `undefined`] |

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
| **409** | Volume is non-empty or in use (use ?force&#x3D;true) |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## getVolume

> VolumeInfo getVolume(name)

Inspect a single volume by name.

Reads the sidecar when present, synthesizes defaults for legacy volumes, and populates &#x60;in_use_by&#x60; when a [&#x60;VolumeUsageSource&#x60;] is wired.  # Errors  - 404 Not Found — no such volume. - 500 Internal — filesystem or parse errors.

### Example

```ts
import {
  Configuration,
  VolumesApi,
} from '@zlayer/api-client';
import type { GetVolumeRequest } from '@zlayer/api-client';

async function example() {
  console.log("🚀 Testing @zlayer/api-client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new VolumesApi(config);

  const body = {
    // string | Volume name
    name: name_example,
  } satisfies GetVolumeRequest;

  try {
    const data = await api.getVolume(body);
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

### Return type

[**VolumeInfo**](VolumeInfo.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Volume metadata |  -  |
| **401** | Unauthorized |  -  |
| **404** | Volume not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## listVolumes

> Array&lt;VolumeInfo&gt; listVolumes()

List all volumes on disk.

Enumerates subdirectories under the volume base directory, reads each sidecar when present, and returns a [&#x60;VolumeInfo&#x60;] for each entry (including &#x60;in_use_by&#x60; when a [&#x60;VolumeUsageSource&#x60;] is wired).  # Errors  Returns an error if the volume directory cannot be read.

### Example

```ts
import {
  Configuration,
  VolumesApi,
} from '@zlayer/api-client';
import type { ListVolumesRequest } from '@zlayer/api-client';

async function example() {
  console.log("🚀 Testing @zlayer/api-client SDK...");
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

[**Array&lt;VolumeInfo&gt;**](VolumeInfo.md)

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

