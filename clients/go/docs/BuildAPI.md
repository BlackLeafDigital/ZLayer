# \BuildAPI

All URIs are relative to *http://localhost*

Method | HTTP request | Description
------------- | ------------- | -------------
[**GetBuildLogs**](BuildAPI.md#GetBuildLogs) | **Get** /api/v1/build/{id}/logs | GET /api/v1/build/{id}/logs Get build logs.
[**GetBuildStatus**](BuildAPI.md#GetBuildStatus) | **Get** /api/v1/build/{id} | GET /api/v1/build/{id} Get build status.
[**ListBuilds**](BuildAPI.md#ListBuilds) | **Get** /api/v1/builds | GET /api/v1/builds List all builds.
[**ListRuntimeTemplates**](BuildAPI.md#ListRuntimeTemplates) | **Get** /api/v1/templates | GET /api/v1/templates List available runtime templates
[**StartBuild**](BuildAPI.md#StartBuild) | **Post** /api/v1/build | POST /api/v1/build Start a new build from multipart upload (Dockerfile + context tarball)
[**StartBuildJson**](BuildAPI.md#StartBuildJson) | **Post** /api/v1/build/json | POST /api/v1/build/json Start a new build from JSON request with a context path on the server.
[**StreamBuild**](BuildAPI.md#StreamBuild) | **Get** /api/v1/build/{id}/stream | GET /api/v1/build/{id}/stream Stream build progress via Server-Sent Events.



## GetBuildLogs

> string GetBuildLogs(ctx, id).Execute()

GET /api/v1/build/{id}/logs Get build logs.



### Example

```go
package main

import (
	"context"
	"fmt"
	"os"
	openapiclient "github.com/GIT_USER_ID/GIT_REPO_ID/zlayer"
)

func main() {
	id := "id_example" // string | Build ID

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.BuildAPI.GetBuildLogs(context.Background(), id).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `BuildAPI.GetBuildLogs``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `GetBuildLogs`: string
	fmt.Fprintf(os.Stdout, "Response from `BuildAPI.GetBuildLogs`: %v\n", resp)
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**id** | **string** | Build ID | 

### Other Parameters

Other parameters are passed through a pointer to a apiGetBuildLogsRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------


### Return type

**string**

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: text/plain

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## GetBuildStatus

> BuildStatus GetBuildStatus(ctx, id).Execute()

GET /api/v1/build/{id} Get build status.



### Example

```go
package main

import (
	"context"
	"fmt"
	"os"
	openapiclient "github.com/GIT_USER_ID/GIT_REPO_ID/zlayer"
)

func main() {
	id := "id_example" // string | Build ID

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.BuildAPI.GetBuildStatus(context.Background(), id).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `BuildAPI.GetBuildStatus``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `GetBuildStatus`: BuildStatus
	fmt.Fprintf(os.Stdout, "Response from `BuildAPI.GetBuildStatus`: %v\n", resp)
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**id** | **string** | Build ID | 

### Other Parameters

Other parameters are passed through a pointer to a apiGetBuildStatusRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------


### Return type

[**BuildStatus**](BuildStatus.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## ListBuilds

> []BuildStatus ListBuilds(ctx).Execute()

GET /api/v1/builds List all builds.



### Example

```go
package main

import (
	"context"
	"fmt"
	"os"
	openapiclient "github.com/GIT_USER_ID/GIT_REPO_ID/zlayer"
)

func main() {

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.BuildAPI.ListBuilds(context.Background()).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `BuildAPI.ListBuilds``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `ListBuilds`: []BuildStatus
	fmt.Fprintf(os.Stdout, "Response from `BuildAPI.ListBuilds`: %v\n", resp)
}
```

### Path Parameters

This endpoint does not need any parameter.

### Other Parameters

Other parameters are passed through a pointer to a apiListBuildsRequest struct via the builder pattern


### Return type

[**[]BuildStatus**](BuildStatus.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## ListRuntimeTemplates

> []TemplateInfo ListRuntimeTemplates(ctx).Execute()

GET /api/v1/templates List available runtime templates

### Example

```go
package main

import (
	"context"
	"fmt"
	"os"
	openapiclient "github.com/GIT_USER_ID/GIT_REPO_ID/zlayer"
)

func main() {

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.BuildAPI.ListRuntimeTemplates(context.Background()).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `BuildAPI.ListRuntimeTemplates``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `ListRuntimeTemplates`: []TemplateInfo
	fmt.Fprintf(os.Stdout, "Response from `BuildAPI.ListRuntimeTemplates`: %v\n", resp)
}
```

### Path Parameters

This endpoint does not need any parameter.

### Other Parameters

Other parameters are passed through a pointer to a apiListRuntimeTemplatesRequest struct via the builder pattern


### Return type

[**[]TemplateInfo**](TemplateInfo.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## StartBuild

> TriggerBuildResponse StartBuild(ctx).Execute()

POST /api/v1/build Start a new build from multipart upload (Dockerfile + context tarball)



### Example

```go
package main

import (
	"context"
	"fmt"
	"os"
	openapiclient "github.com/GIT_USER_ID/GIT_REPO_ID/zlayer"
)

func main() {

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.BuildAPI.StartBuild(context.Background()).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `BuildAPI.StartBuild``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `StartBuild`: TriggerBuildResponse
	fmt.Fprintf(os.Stdout, "Response from `BuildAPI.StartBuild`: %v\n", resp)
}
```

### Path Parameters

This endpoint does not need any parameter.

### Other Parameters

Other parameters are passed through a pointer to a apiStartBuildRequest struct via the builder pattern


### Return type

[**TriggerBuildResponse**](TriggerBuildResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: multipart/form-data
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## StartBuildJson

> TriggerBuildResponse StartBuildJson(ctx).BuildRequestWithContext(buildRequestWithContext).Execute()

POST /api/v1/build/json Start a new build from JSON request with a context path on the server.



### Example

```go
package main

import (
	"context"
	"fmt"
	"os"
	openapiclient "github.com/GIT_USER_ID/GIT_REPO_ID/zlayer"
)

func main() {
	buildRequestWithContext := *openapiclient.NewBuildRequestWithContext("ContextPath_example") // BuildRequestWithContext | 

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.BuildAPI.StartBuildJson(context.Background()).BuildRequestWithContext(buildRequestWithContext).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `BuildAPI.StartBuildJson``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `StartBuildJson`: TriggerBuildResponse
	fmt.Fprintf(os.Stdout, "Response from `BuildAPI.StartBuildJson`: %v\n", resp)
}
```

### Path Parameters



### Other Parameters

Other parameters are passed through a pointer to a apiStartBuildJsonRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
 **buildRequestWithContext** | [**BuildRequestWithContext**](BuildRequestWithContext.md) |  | 

### Return type

[**TriggerBuildResponse**](TriggerBuildResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: application/json
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## StreamBuild

> StreamBuild(ctx, id).Execute()

GET /api/v1/build/{id}/stream Stream build progress via Server-Sent Events.



### Example

```go
package main

import (
	"context"
	"fmt"
	"os"
	openapiclient "github.com/GIT_USER_ID/GIT_REPO_ID/zlayer"
)

func main() {
	id := "id_example" // string | Build ID

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	r, err := apiClient.BuildAPI.StreamBuild(context.Background(), id).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `BuildAPI.StreamBuild``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**id** | **string** | Build ID | 

### Other Parameters

Other parameters are passed through a pointer to a apiStreamBuildRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------


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

