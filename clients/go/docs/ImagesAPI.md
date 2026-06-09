# \ImagesAPI

All URIs are relative to *http://localhost*

Method | HTTP request | Description
------------- | ------------- | -------------
[**ListImagesHandler**](ImagesAPI.md#ListImagesHandler) | **Get** /api/v1/images | List all cached images known to the runtime.
[**PruneImagesHandler**](ImagesAPI.md#PruneImagesHandler) | **Post** /api/v1/system/prune | Prune dangling / unused images from the runtime&#39;s cache.
[**PullImageHandler**](ImagesAPI.md#PullImageHandler) | **Post** /api/v1/images/pull | Pull an OCI image into the runtime&#39;s local cache.
[**RemoveImageHandler**](ImagesAPI.md#RemoveImageHandler) | **Delete** /api/v1/images/{image} | Remove an image from the runtime&#39;s cache.
[**TagImageHandler**](ImagesAPI.md#TagImageHandler) | **Post** /api/v1/images/tag | Create a new tag pointing at an existing image.



## ListImagesHandler

> []ImageInfoDto ListImagesHandler(ctx).Execute()

List all cached images known to the runtime.



### Example

```go
package main

import (
	"context"
	"fmt"
	"os"
	openapiclient "github.com/BlackLeafDigital/ZLayer/clients/go"
)

func main() {

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.ImagesAPI.ListImagesHandler(context.Background()).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `ImagesAPI.ListImagesHandler``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `ListImagesHandler`: []ImageInfoDto
	fmt.Fprintf(os.Stdout, "Response from `ImagesAPI.ListImagesHandler`: %v\n", resp)
}
```

### Path Parameters

This endpoint does not need any parameter.

### Other Parameters

Other parameters are passed through a pointer to a apiListImagesHandlerRequest struct via the builder pattern


### Return type

[**[]ImageInfoDto**](ImageInfoDto.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## PruneImagesHandler

> PruneResultDto PruneImagesHandler(ctx).Execute()

Prune dangling / unused images from the runtime's cache.



### Example

```go
package main

import (
	"context"
	"fmt"
	"os"
	openapiclient "github.com/BlackLeafDigital/ZLayer/clients/go"
)

func main() {

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.ImagesAPI.PruneImagesHandler(context.Background()).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `ImagesAPI.PruneImagesHandler``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `PruneImagesHandler`: PruneResultDto
	fmt.Fprintf(os.Stdout, "Response from `ImagesAPI.PruneImagesHandler`: %v\n", resp)
}
```

### Path Parameters

This endpoint does not need any parameter.

### Other Parameters

Other parameters are passed through a pointer to a apiPruneImagesHandlerRequest struct via the builder pattern


### Return type

[**PruneResultDto**](PruneResultDto.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## PullImageHandler

> PullImageResponse PullImageHandler(ctx).PullImageRequest(pullImageRequest).Stream(stream).Execute()

Pull an OCI image into the runtime's local cache.



### Example

```go
package main

import (
	"context"
	"fmt"
	"os"
	openapiclient "github.com/BlackLeafDigital/ZLayer/clients/go"
)

func main() {
	pullImageRequest := *openapiclient.NewPullImageRequest("Reference_example") // PullImageRequest | 
	stream := true // bool | When `true`, stream NDJSON `PullProgressDto` events instead of a single JSON response. Defaults to `false` (snapshot pull). (optional)

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.ImagesAPI.PullImageHandler(context.Background()).PullImageRequest(pullImageRequest).Stream(stream).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `ImagesAPI.PullImageHandler``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `PullImageHandler`: PullImageResponse
	fmt.Fprintf(os.Stdout, "Response from `ImagesAPI.PullImageHandler`: %v\n", resp)
}
```

### Path Parameters



### Other Parameters

Other parameters are passed through a pointer to a apiPullImageHandlerRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
 **pullImageRequest** | [**PullImageRequest**](PullImageRequest.md) |  | 
 **stream** | **bool** | When &#x60;true&#x60;, stream NDJSON &#x60;PullProgressDto&#x60; events instead of a single JSON response. Defaults to &#x60;false&#x60; (snapshot pull). | 

### Return type

[**PullImageResponse**](PullImageResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: application/json
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## RemoveImageHandler

> RemoveImageHandler(ctx, image).Force(force).Execute()

Remove an image from the runtime's cache.



### Example

```go
package main

import (
	"context"
	"fmt"
	"os"
	openapiclient "github.com/BlackLeafDigital/ZLayer/clients/go"
)

func main() {
	image := "image_example" // string | Image reference (URL-encoded)
	force := true // bool | Force removal even if the image is referenced by containers. (optional)

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	r, err := apiClient.ImagesAPI.RemoveImageHandler(context.Background(), image).Force(force).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `ImagesAPI.RemoveImageHandler``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**image** | **string** | Image reference (URL-encoded) | 

### Other Parameters

Other parameters are passed through a pointer to a apiRemoveImageHandlerRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------

 **force** | **bool** | Force removal even if the image is referenced by containers. | 

### Return type

 (empty response body)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: Not defined

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## TagImageHandler

> TagImageHandler(ctx).TagImageRequest(tagImageRequest).Execute()

Create a new tag pointing at an existing image.



### Example

```go
package main

import (
	"context"
	"fmt"
	"os"
	openapiclient "github.com/BlackLeafDigital/ZLayer/clients/go"
)

func main() {
	tagImageRequest := *openapiclient.NewTagImageRequest("Source_example", "Target_example") // TagImageRequest | 

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	r, err := apiClient.ImagesAPI.TagImageHandler(context.Background()).TagImageRequest(tagImageRequest).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `ImagesAPI.TagImageHandler``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
}
```

### Path Parameters



### Other Parameters

Other parameters are passed through a pointer to a apiTagImageHandlerRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
 **tagImageRequest** | [**TagImageRequest**](TagImageRequest.md) |  | 

### Return type

 (empty response body)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: application/json
- **Accept**: Not defined

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)

