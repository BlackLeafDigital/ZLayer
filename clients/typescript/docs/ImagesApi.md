# ImagesApi

All URIs are relative to *http://localhost*

| Method | HTTP request | Description |
|------------- | ------------- | -------------|
| [**listImagesHandler**](ImagesApi.md#listimageshandler) | **GET** /api/v1/images | List all cached images known to the runtime. |
| [**pruneImagesHandler**](ImagesApi.md#pruneimageshandler) | **POST** /api/v1/system/prune | Prune dangling / unused images from the runtime\&#39;s cache. |
| [**pullImageHandler**](ImagesApi.md#pullimagehandler) | **POST** /api/v1/images/pull | Pull an OCI image into the runtime\&#39;s local cache. |
| [**removeImageHandler**](ImagesApi.md#removeimagehandler) | **DELETE** /api/v1/images/{image} | Remove an image from the runtime\&#39;s cache. |
| [**tagImageHandler**](ImagesApi.md#tagimagehandler) | **POST** /api/v1/images/tag | Create a new tag pointing at an existing image. |



## listImagesHandler

> Array&lt;ImageInfoDto&gt; listImagesHandler()

List all cached images known to the runtime.

# Errors  Returns an error if authentication fails or the runtime cannot enumerate its image cache (for example, when the backend does not implement &#x60;list_images&#x60;).

### Example

```ts
import {
  Configuration,
  ImagesApi,
} from '@zlayer/client';
import type { ListImagesHandlerRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new ImagesApi(config);

  try {
    const data = await api.listImagesHandler();
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

[**Array&lt;ImageInfoDto&gt;**](ImageInfoDto.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | List of cached images |  -  |
| **401** | Unauthorized |  -  |
| **501** | Runtime does not support image listing |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## pruneImagesHandler

> PruneResultDto pruneImagesHandler()

Prune dangling / unused images from the runtime\&#39;s cache.

# Errors  Returns an error if authentication fails or the runtime backend does not support pruning.

### Example

```ts
import {
  Configuration,
  ImagesApi,
} from '@zlayer/client';
import type { PruneImagesHandlerRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new ImagesApi(config);

  try {
    const data = await api.pruneImagesHandler();
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

[**PruneResultDto**](PruneResultDto.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Prune result |  -  |
| **401** | Unauthorized |  -  |
| **501** | Runtime does not support pruning |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## pullImageHandler

> PullImageResponse pullImageHandler(pullImageRequest)

Pull an OCI image into the runtime\&#39;s local cache.

This is a blocking pull: the handler returns only after the image is resolved and stored locally (or the pull fails). When &#x60;pull_policy&#x60; is omitted the default is &#x60;\&quot;always\&quot;&#x60;, matching Docker-compat semantics for &#x60;POST /images/create&#x60;. On success, the response echoes the reference and best-effort &#x60;digest&#x60;/&#x60;size_bytes&#x60; resolved via &#x60;list_images()&#x60;.  # Errors  Returns an error if authentication fails, the reference is empty, the image cannot be pulled, or the user lacks the operator role.

### Example

```ts
import {
  Configuration,
  ImagesApi,
} from '@zlayer/client';
import type { PullImageHandlerRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new ImagesApi(config);

  const body = {
    // PullImageRequest
    pullImageRequest: ...,
  } satisfies PullImageHandlerRequest;

  try {
    const data = await api.pullImageHandler(body);
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
| **pullImageRequest** | [PullImageRequest](PullImageRequest.md) |  | |

### Return type

[**PullImageResponse**](PullImageResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: `application/json`
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Image pulled |  -  |
| **400** | Invalid request |  -  |
| **401** | Unauthorized |  -  |
| **403** | Forbidden - operator role required |  -  |
| **500** | Pull failed |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## removeImageHandler

> removeImageHandler(image, force)

Remove an image from the runtime\&#39;s cache.

# Errors  Returns an error if authentication fails, the image cannot be found, or the runtime backend does not support image removal.

### Example

```ts
import {
  Configuration,
  ImagesApi,
} from '@zlayer/client';
import type { RemoveImageHandlerRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new ImagesApi(config);

  const body = {
    // string | Image reference (URL-encoded)
    image: image_example,
    // boolean | Force removal even if the image is referenced by containers. (optional)
    force: true,
  } satisfies RemoveImageHandlerRequest;

  try {
    const data = await api.removeImageHandler(body);
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
| **image** | `string` | Image reference (URL-encoded) | [Defaults to `undefined`] |
| **force** | `boolean` | Force removal even if the image is referenced by containers. | [Optional] [Defaults to `undefined`] |

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
| **204** | Image removed |  -  |
| **401** | Unauthorized |  -  |
| **404** | Image not found |  -  |
| **501** | Runtime does not support image removal |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## tagImageHandler

> tagImageHandler(tagImageRequest)

Create a new tag pointing at an existing image.

Docker-compat &#x60;POST /api/v1/images/tag&#x60;: takes &#x60;{ source, target }&#x60; and asks the runtime to make &#x60;target&#x60; resolve to the same content as &#x60;source&#x60;. Both references must be non-empty; &#x60;target&#x60; is split on the last &#x60;:&#x60; into repository + tag (defaulting tag to &#x60;latest&#x60;).  # Errors  Returns &#x60;400&#x60; if the request body is malformed, &#x60;404&#x60; if the source image is not in the cache, &#x60;403&#x60; if the caller lacks the &#x60;operator&#x60; role, &#x60;501&#x60; if the runtime does not support tagging, and &#x60;500&#x60; for other runtime errors.

### Example

```ts
import {
  Configuration,
  ImagesApi,
} from '@zlayer/client';
import type { TagImageHandlerRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new ImagesApi(config);

  const body = {
    // TagImageRequest
    tagImageRequest: ...,
  } satisfies TagImageHandlerRequest;

  try {
    const data = await api.tagImageHandler(body);
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
| **tagImageRequest** | [TagImageRequest](TagImageRequest.md) |  | |

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
| **204** | Tag created |  -  |
| **400** | Invalid request |  -  |
| **401** | Unauthorized |  -  |
| **403** | Forbidden - operator role required |  -  |
| **404** | Source image not found |  -  |
| **500** | Internal error |  -  |
| **501** | Runtime does not support tagging |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)

