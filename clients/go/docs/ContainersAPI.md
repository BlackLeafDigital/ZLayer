# \ContainersAPI

All URIs are relative to *http://localhost*

Method | HTTP request | Description
------------- | ------------- | -------------
[**ArchiveGet**](ContainersAPI.md#ArchiveGet) | **Get** /api/v1/containers/{id}/archive | &#x60;GET /api/v1/containers/{id}/archive?path&#x3D;&lt;...&gt;&#x60; — stream a TAR archive of the requested file or directory inside the container.
[**ArchiveHead**](ContainersAPI.md#ArchiveHead) | **Head** /api/v1/containers/{id}/archive | &#x60;HEAD /api/v1/containers/{id}/archive?path&#x3D;&lt;...&gt;&#x60; — return path-stat metadata in the &#x60;X-Docker-Container-Path-Stat&#x60; header without materializing the TAR archive.
[**ArchivePut**](ContainersAPI.md#ArchivePut) | **Put** /api/v1/containers/{id}/archive | &#x60;PUT /api/v1/containers/{id}/archive?path&#x3D;&lt;...&gt;&#x60; — extract a TAR archive into the container at the given path.
[**ChangesContainer**](ContainersAPI.md#ChangesContainer) | **Get** /api/v1/containers/{id}/changes | Report changes to the container&#39;s filesystem.
[**CreateContainer**](ContainersAPI.md#CreateContainer) | **Post** /api/v1/containers | Create and start a container (public, JWT-authenticated entry point).
[**DeleteContainer**](ContainersAPI.md#DeleteContainer) | **Delete** /api/v1/containers/{id} | Stop and remove a container.
[**ExecInContainer**](ContainersAPI.md#ExecInContainer) | **Post** /api/v1/containers/{id}/exec | Execute a command in a running container.
[**GetContainer**](ContainersAPI.md#GetContainer) | **Get** /api/v1/containers/{id} | Get details for a specific container.
[**GetContainerLogs**](ContainersAPI.md#GetContainerLogs) | **Get** /api/v1/containers/{id}/logs | Stream container logs.
[**GetContainerStats**](ContainersAPI.md#GetContainerStats) | **Get** /api/v1/containers/{id}/stats | Stream container resource statistics.
[**KillContainer**](ContainersAPI.md#KillContainer) | **Post** /api/v1/containers/{id}/kill | Send a signal to a running container.
[**ListContainers**](ContainersAPI.md#ListContainers) | **Get** /api/v1/containers | List standalone containers.
[**PauseContainer**](ContainersAPI.md#PauseContainer) | **Post** /api/v1/containers/{id}/pause | Pause a running container by freezing its cgroup.
[**PortContainer**](ContainersAPI.md#PortContainer) | **Get** /api/v1/containers/{id}/port | Report the published port mappings for a container.
[**PruneContainers**](ContainersAPI.md#PruneContainers) | **Post** /api/v1/containers/prune | Prune stopped containers from the runtime.
[**RenameContainer**](ContainersAPI.md#RenameContainer) | **Post** /api/v1/containers/{id}/rename | Rename a standalone container.
[**RestartContainer**](ContainersAPI.md#RestartContainer) | **Post** /api/v1/containers/{id}/restart | Restart a container: stop then start.
[**StartContainer**](ContainersAPI.md#StartContainer) | **Post** /api/v1/containers/{id}/start | Start a previously-created container.
[**StopContainer**](ContainersAPI.md#StopContainer) | **Post** /api/v1/containers/{id}/stop | Stop a running container.
[**TopContainer**](ContainersAPI.md#TopContainer) | **Get** /api/v1/containers/{id}/top | List the processes running inside a container.
[**UnpauseContainer**](ContainersAPI.md#UnpauseContainer) | **Post** /api/v1/containers/{id}/unpause | Resume a previously-paused container.
[**UpdateContainer**](ContainersAPI.md#UpdateContainer) | **Post** /api/v1/containers/{id}/update | Update a standalone container&#39;s resource limits and/or restart policy.
[**WaitContainer**](ContainersAPI.md#WaitContainer) | **Get** /api/v1/containers/{id}/wait | Wait for a container to exit and return its exit code.
[**WaitContainerPost**](ContainersAPI.md#WaitContainerPost) | **Post** /api/v1/containers/{id}/wait | Wait for a container to exit and return Docker-shaped JSON.



## ArchiveGet

> ArchiveGet(ctx, id).Path(path).Execute()

`GET /api/v1/containers/{id}/archive?path=<...>` — stream a TAR archive of the requested file or directory inside the container.



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
	id := "id_example" // string | Container identifier
	path := "path_example" // string | Container-side path to archive

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	r, err := apiClient.ContainersAPI.ArchiveGet(context.Background(), id).Path(path).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `ContainersAPI.ArchiveGet``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**id** | **string** | Container identifier | 

### Other Parameters

Other parameters are passed through a pointer to a apiArchiveGetRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------

 **path** | **string** | Container-side path to archive | 

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


## ArchiveHead

> ArchiveHead(ctx, id).Path(path).Execute()

`HEAD /api/v1/containers/{id}/archive?path=<...>` — return path-stat metadata in the `X-Docker-Container-Path-Stat` header without materializing the TAR archive.



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
	id := "id_example" // string | Container identifier
	path := "path_example" // string | Container-side path to stat

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	r, err := apiClient.ContainersAPI.ArchiveHead(context.Background(), id).Path(path).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `ContainersAPI.ArchiveHead``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**id** | **string** | Container identifier | 

### Other Parameters

Other parameters are passed through a pointer to a apiArchiveHeadRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------

 **path** | **string** | Container-side path to stat | 

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


## ArchivePut

> ArchivePut(ctx, id).Path(path).RequestBody(requestBody).Execute()

`PUT /api/v1/containers/{id}/archive?path=<...>` — extract a TAR archive into the container at the given path.



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
	id := "id_example" // string | Container identifier
	path := "path_example" // string | Container-side path to extract into
	requestBody := []int32{int32(123)} // []int32 | Uncompressed TAR archive to extract into the container

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	r, err := apiClient.ContainersAPI.ArchivePut(context.Background(), id).Path(path).RequestBody(requestBody).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `ContainersAPI.ArchivePut``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**id** | **string** | Container identifier | 

### Other Parameters

Other parameters are passed through a pointer to a apiArchivePutRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------

 **path** | **string** | Container-side path to extract into | 
 **requestBody** | **[]int32** | Uncompressed TAR archive to extract into the container | 

### Return type

 (empty response body)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: application/x-tar
- **Accept**: Not defined

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## ChangesContainer

> []ContainerChangeEntry ChangesContainer(ctx, id).Execute()

Report changes to the container's filesystem.



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
	id := "id_example" // string | Container identifier

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.ContainersAPI.ChangesContainer(context.Background(), id).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `ContainersAPI.ChangesContainer``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `ChangesContainer`: []ContainerChangeEntry
	fmt.Fprintf(os.Stdout, "Response from `ContainersAPI.ChangesContainer`: %v\n", resp)
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**id** | **string** | Container identifier | 

### Other Parameters

Other parameters are passed through a pointer to a apiChangesContainerRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------


### Return type

[**[]ContainerChangeEntry**](ContainerChangeEntry.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## CreateContainer

> ContainerInfo CreateContainer(ctx).CreateContainerRequest(createContainerRequest).Execute()

Create and start a container (public, JWT-authenticated entry point).



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
	createContainerRequest := *openapiclient.NewCreateContainerRequest("Image_example") // CreateContainerRequest | 

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.ContainersAPI.CreateContainer(context.Background()).CreateContainerRequest(createContainerRequest).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `ContainersAPI.CreateContainer``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `CreateContainer`: ContainerInfo
	fmt.Fprintf(os.Stdout, "Response from `ContainersAPI.CreateContainer`: %v\n", resp)
}
```

### Path Parameters



### Other Parameters

Other parameters are passed through a pointer to a apiCreateContainerRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
 **createContainerRequest** | [**CreateContainerRequest**](CreateContainerRequest.md) |  | 

### Return type

[**ContainerInfo**](ContainerInfo.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: application/json
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## DeleteContainer

> DeleteContainer(ctx, id).Execute()

Stop and remove a container.



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
	id := "id_example" // string | Container identifier

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	r, err := apiClient.ContainersAPI.DeleteContainer(context.Background(), id).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `ContainersAPI.DeleteContainer``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**id** | **string** | Container identifier | 

### Other Parameters

Other parameters are passed through a pointer to a apiDeleteContainerRequest struct via the builder pattern


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


## ExecInContainer

> ContainerExecResponse ExecInContainer(ctx, id).ContainerExecRequest(containerExecRequest).Stream(stream).Execute()

Execute a command in a running container.



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
	id := "id_example" // string | Container identifier
	containerExecRequest := *openapiclient.NewContainerExecRequest([]string{"Command_example"}) // ContainerExecRequest | 
	stream := true // bool | Stream exec events as SSE instead of returning a buffered JSON body. (optional)

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.ContainersAPI.ExecInContainer(context.Background(), id).ContainerExecRequest(containerExecRequest).Stream(stream).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `ContainersAPI.ExecInContainer``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `ExecInContainer`: ContainerExecResponse
	fmt.Fprintf(os.Stdout, "Response from `ContainersAPI.ExecInContainer`: %v\n", resp)
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**id** | **string** | Container identifier | 

### Other Parameters

Other parameters are passed through a pointer to a apiExecInContainerRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------

 **containerExecRequest** | [**ContainerExecRequest**](ContainerExecRequest.md) |  | 
 **stream** | **bool** | Stream exec events as SSE instead of returning a buffered JSON body. | 

### Return type

[**ContainerExecResponse**](ContainerExecResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: application/json
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## GetContainer

> ContainerInfo GetContainer(ctx, id).Execute()

Get details for a specific container.



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
	id := "id_example" // string | Container identifier

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.ContainersAPI.GetContainer(context.Background(), id).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `ContainersAPI.GetContainer``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `GetContainer`: ContainerInfo
	fmt.Fprintf(os.Stdout, "Response from `ContainersAPI.GetContainer`: %v\n", resp)
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**id** | **string** | Container identifier | 

### Other Parameters

Other parameters are passed through a pointer to a apiGetContainerRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------


### Return type

[**ContainerInfo**](ContainerInfo.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## GetContainerLogs

> string GetContainerLogs(ctx, id).Tail(tail).Follow(follow).Since(since).Until(until).Timestamps(timestamps).Stdout(stdout).Stderr(stderr).Format(format).Execute()

Stream container logs.



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
	id := "id_example" // string | Container identifier
	tail := int32(56) // int32 | Number of tail lines to return. `0` and \"all\" map to \"everything available\"; otherwise the runtime ships the last `tail` lines before the live stream begins. (optional)
	follow := true // bool | Follow logs after the current end-of-buffer marker. (optional)
	since := int64(789) // int64 | Earliest log timestamp to include (Unix seconds). `None` means no lower bound. (optional)
	until := int64(789) // int64 | Latest log timestamp to include (Unix seconds). `None` means no upper bound. (optional)
	timestamps := true // bool | When `true`, the runtime is asked to populate per-chunk timestamps so the wire-format includes them. (optional)
	stdout := true // bool | Include stdout chunks. When neither `stdout` nor `stderr` is set, the handler defaults both to `true` (Docker parity). (optional)
	stderr := true // bool | Include stderr chunks. See [`ContainerLogQuery::stdout`] for the \"neither set\" default behavior. (optional)
	format := openapiclient.ContainerLogFormat("json") // ContainerLogFormat | Wire format for the streamed body. `\"json\"` (the default) emits one NDJSON `LogChunk` per line; `\"raw\"` emits Docker's multiplexed stdcopy frames (`application/vnd.docker.raw-stream`). (optional)

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.ContainersAPI.GetContainerLogs(context.Background(), id).Tail(tail).Follow(follow).Since(since).Until(until).Timestamps(timestamps).Stdout(stdout).Stderr(stderr).Format(format).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `ContainersAPI.GetContainerLogs``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `GetContainerLogs`: string
	fmt.Fprintf(os.Stdout, "Response from `ContainersAPI.GetContainerLogs`: %v\n", resp)
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**id** | **string** | Container identifier | 

### Other Parameters

Other parameters are passed through a pointer to a apiGetContainerLogsRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------

 **tail** | **int32** | Number of tail lines to return. &#x60;0&#x60; and \&quot;all\&quot; map to \&quot;everything available\&quot;; otherwise the runtime ships the last &#x60;tail&#x60; lines before the live stream begins. | 
 **follow** | **bool** | Follow logs after the current end-of-buffer marker. | 
 **since** | **int64** | Earliest log timestamp to include (Unix seconds). &#x60;None&#x60; means no lower bound. | 
 **until** | **int64** | Latest log timestamp to include (Unix seconds). &#x60;None&#x60; means no upper bound. | 
 **timestamps** | **bool** | When &#x60;true&#x60;, the runtime is asked to populate per-chunk timestamps so the wire-format includes them. | 
 **stdout** | **bool** | Include stdout chunks. When neither &#x60;stdout&#x60; nor &#x60;stderr&#x60; is set, the handler defaults both to &#x60;true&#x60; (Docker parity). | 
 **stderr** | **bool** | Include stderr chunks. See [&#x60;ContainerLogQuery::stdout&#x60;] for the \&quot;neither set\&quot; default behavior. | 
 **format** | [**ContainerLogFormat**](ContainerLogFormat.md) | Wire format for the streamed body. &#x60;\&quot;json\&quot;&#x60; (the default) emits one NDJSON &#x60;LogChunk&#x60; per line; &#x60;\&quot;raw\&quot;&#x60; emits Docker&#39;s multiplexed stdcopy frames (&#x60;application/vnd.docker.raw-stream&#x60;). | 

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


## GetContainerStats

> ContainerStatsResponse GetContainerStats(ctx, id).Stream(stream).Interval(interval).Execute()

Stream container resource statistics.



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
	id := "id_example" // string | Container identifier
	stream := true // bool | Stream periodic samples as SSE events instead of a one-shot JSON response. (optional)
	interval := int32(56) // int32 | Sample cadence in seconds (only used when `stream=true`). Clamped to `[1, 60]`. Defaults to `2` seconds. (optional)

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.ContainersAPI.GetContainerStats(context.Background(), id).Stream(stream).Interval(interval).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `ContainersAPI.GetContainerStats``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `GetContainerStats`: ContainerStatsResponse
	fmt.Fprintf(os.Stdout, "Response from `ContainersAPI.GetContainerStats`: %v\n", resp)
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**id** | **string** | Container identifier | 

### Other Parameters

Other parameters are passed through a pointer to a apiGetContainerStatsRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------

 **stream** | **bool** | Stream periodic samples as SSE events instead of a one-shot JSON response. | 
 **interval** | **int32** | Sample cadence in seconds (only used when &#x60;stream&#x3D;true&#x60;). Clamped to &#x60;[1, 60]&#x60;. Defaults to &#x60;2&#x60; seconds. | 

### Return type

[**ContainerStatsResponse**](ContainerStatsResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## KillContainer

> KillContainer(ctx, id).KillContainerRequest(killContainerRequest).Execute()

Send a signal to a running container.



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
	id := "id_example" // string | Container identifier
	killContainerRequest := *openapiclient.NewKillContainerRequest() // KillContainerRequest | 

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	r, err := apiClient.ContainersAPI.KillContainer(context.Background(), id).KillContainerRequest(killContainerRequest).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `ContainersAPI.KillContainer``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**id** | **string** | Container identifier | 

### Other Parameters

Other parameters are passed through a pointer to a apiKillContainerRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------

 **killContainerRequest** | [**KillContainerRequest**](KillContainerRequest.md) |  | 

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


## ListContainers

> []ContainerInfo ListContainers(ctx).Label(label).Execute()

List standalone containers.



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
	label := "label_example" // string | Filter by label (key=value format) (optional)

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.ContainersAPI.ListContainers(context.Background()).Label(label).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `ContainersAPI.ListContainers``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `ListContainers`: []ContainerInfo
	fmt.Fprintf(os.Stdout, "Response from `ContainersAPI.ListContainers`: %v\n", resp)
}
```

### Path Parameters



### Other Parameters

Other parameters are passed through a pointer to a apiListContainersRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
 **label** | **string** | Filter by label (key&#x3D;value format) | 

### Return type

[**[]ContainerInfo**](ContainerInfo.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## PauseContainer

> PauseContainer(ctx, id).Execute()

Pause a running container by freezing its cgroup.



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
	id := "id_example" // string | Container identifier

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	r, err := apiClient.ContainersAPI.PauseContainer(context.Background(), id).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `ContainersAPI.PauseContainer``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**id** | **string** | Container identifier | 

### Other Parameters

Other parameters are passed through a pointer to a apiPauseContainerRequest struct via the builder pattern


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


## PortContainer

> ContainerPortResponse PortContainer(ctx, id).Execute()

Report the published port mappings for a container.



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
	id := "id_example" // string | Container identifier

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.ContainersAPI.PortContainer(context.Background(), id).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `ContainersAPI.PortContainer``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `PortContainer`: ContainerPortResponse
	fmt.Fprintf(os.Stdout, "Response from `ContainersAPI.PortContainer`: %v\n", resp)
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**id** | **string** | Container identifier | 

### Other Parameters

Other parameters are passed through a pointer to a apiPortContainerRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------


### Return type

[**ContainerPortResponse**](ContainerPortResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## PruneContainers

> ContainerPruneResponse PruneContainers(ctx).Execute()

Prune stopped containers from the runtime.



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
	resp, r, err := apiClient.ContainersAPI.PruneContainers(context.Background()).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `ContainersAPI.PruneContainers``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `PruneContainers`: ContainerPruneResponse
	fmt.Fprintf(os.Stdout, "Response from `ContainersAPI.PruneContainers`: %v\n", resp)
}
```

### Path Parameters

This endpoint does not need any parameter.

### Other Parameters

Other parameters are passed through a pointer to a apiPruneContainersRequest struct via the builder pattern


### Return type

[**ContainerPruneResponse**](ContainerPruneResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## RenameContainer

> RenameContainer(ctx, id).Name(name).Execute()

Rename a standalone container.



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
	id := "id_example" // string | Container identifier
	name := "name_example" // string | New human-readable name to assign to the container. Required. (optional)

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	r, err := apiClient.ContainersAPI.RenameContainer(context.Background(), id).Name(name).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `ContainersAPI.RenameContainer``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**id** | **string** | Container identifier | 

### Other Parameters

Other parameters are passed through a pointer to a apiRenameContainerRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------

 **name** | **string** | New human-readable name to assign to the container. Required. | 

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


## RestartContainer

> RestartContainer(ctx, id).RestartContainerRequest(restartContainerRequest).Execute()

Restart a container: stop then start.



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
	id := "id_example" // string | Container identifier
	restartContainerRequest := *openapiclient.NewRestartContainerRequest() // RestartContainerRequest | 

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	r, err := apiClient.ContainersAPI.RestartContainer(context.Background(), id).RestartContainerRequest(restartContainerRequest).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `ContainersAPI.RestartContainer``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**id** | **string** | Container identifier | 

### Other Parameters

Other parameters are passed through a pointer to a apiRestartContainerRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------

 **restartContainerRequest** | [**RestartContainerRequest**](RestartContainerRequest.md) |  | 

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


## StartContainer

> StartContainer(ctx, id).Execute()

Start a previously-created container.



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
	id := "id_example" // string | Container identifier

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	r, err := apiClient.ContainersAPI.StartContainer(context.Background(), id).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `ContainersAPI.StartContainer``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**id** | **string** | Container identifier | 

### Other Parameters

Other parameters are passed through a pointer to a apiStartContainerRequest struct via the builder pattern


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


## StopContainer

> StopContainer(ctx, id).StopContainerRequest(stopContainerRequest).Execute()

Stop a running container.



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
	id := "id_example" // string | Container identifier
	stopContainerRequest := *openapiclient.NewStopContainerRequest() // StopContainerRequest | 

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	r, err := apiClient.ContainersAPI.StopContainer(context.Background(), id).StopContainerRequest(stopContainerRequest).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `ContainersAPI.StopContainer``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**id** | **string** | Container identifier | 

### Other Parameters

Other parameters are passed through a pointer to a apiStopContainerRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------

 **stopContainerRequest** | [**StopContainerRequest**](StopContainerRequest.md) |  | 

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


## TopContainer

> ContainerTopResponse TopContainer(ctx, id).PsArgs(psArgs).Execute()

List the processes running inside a container.



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
	id := "id_example" // string | Container identifier
	psArgs := "psArgs_example" // string | `ps`-style argument string, e.g. `\"aux\"` or `\"-eo pid,user,cmd\"`. Empty / omitted means \"use the runtime's defaults\". (optional)

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.ContainersAPI.TopContainer(context.Background(), id).PsArgs(psArgs).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `ContainersAPI.TopContainer``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `TopContainer`: ContainerTopResponse
	fmt.Fprintf(os.Stdout, "Response from `ContainersAPI.TopContainer`: %v\n", resp)
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**id** | **string** | Container identifier | 

### Other Parameters

Other parameters are passed through a pointer to a apiTopContainerRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------

 **psArgs** | **string** | &#x60;ps&#x60;-style argument string, e.g. &#x60;\&quot;aux\&quot;&#x60; or &#x60;\&quot;-eo pid,user,cmd\&quot;&#x60;. Empty / omitted means \&quot;use the runtime&#39;s defaults\&quot;. | 

### Return type

[**ContainerTopResponse**](ContainerTopResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## UnpauseContainer

> UnpauseContainer(ctx, id).Execute()

Resume a previously-paused container.



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
	id := "id_example" // string | Container identifier

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	r, err := apiClient.ContainersAPI.UnpauseContainer(context.Background(), id).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `ContainersAPI.UnpauseContainer``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**id** | **string** | Container identifier | 

### Other Parameters

Other parameters are passed through a pointer to a apiUnpauseContainerRequest struct via the builder pattern


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


## UpdateContainer

> ContainerUpdateResponse UpdateContainer(ctx, id).ContainerUpdateRequest(containerUpdateRequest).Execute()

Update a standalone container's resource limits and/or restart policy.



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
	id := "id_example" // string | Container identifier
	containerUpdateRequest := *openapiclient.NewContainerUpdateRequest() // ContainerUpdateRequest | 

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.ContainersAPI.UpdateContainer(context.Background(), id).ContainerUpdateRequest(containerUpdateRequest).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `ContainersAPI.UpdateContainer``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `UpdateContainer`: ContainerUpdateResponse
	fmt.Fprintf(os.Stdout, "Response from `ContainersAPI.UpdateContainer`: %v\n", resp)
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**id** | **string** | Container identifier | 

### Other Parameters

Other parameters are passed through a pointer to a apiUpdateContainerRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------

 **containerUpdateRequest** | [**ContainerUpdateRequest**](ContainerUpdateRequest.md) |  | 

### Return type

[**ContainerUpdateResponse**](ContainerUpdateResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: application/json
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## WaitContainer

> ContainerWaitResponse WaitContainer(ctx, id).Execute()

Wait for a container to exit and return its exit code.



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
	id := "id_example" // string | Container identifier

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.ContainersAPI.WaitContainer(context.Background(), id).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `ContainersAPI.WaitContainer``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `WaitContainer`: ContainerWaitResponse
	fmt.Fprintf(os.Stdout, "Response from `ContainersAPI.WaitContainer`: %v\n", resp)
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**id** | **string** | Container identifier | 

### Other Parameters

Other parameters are passed through a pointer to a apiWaitContainerRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------


### Return type

[**ContainerWaitResponse**](ContainerWaitResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## WaitContainerPost

> ContainerWaitDockerResponse WaitContainerPost(ctx, id).Condition(condition).Execute()

Wait for a container to exit and return Docker-shaped JSON.



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
	id := "id_example" // string | Container identifier
	condition := "condition_example" // string | One of `\"not-running\"` (default), `\"next-exit\"`, or `\"removed\"`. Matches Docker's `/containers/{id}/wait` semantics. Omitted values default to `\"not-running\"`. (optional)

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.ContainersAPI.WaitContainerPost(context.Background(), id).Condition(condition).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `ContainersAPI.WaitContainerPost``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `WaitContainerPost`: ContainerWaitDockerResponse
	fmt.Fprintf(os.Stdout, "Response from `ContainersAPI.WaitContainerPost`: %v\n", resp)
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**id** | **string** | Container identifier | 

### Other Parameters

Other parameters are passed through a pointer to a apiWaitContainerPostRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------

 **condition** | **string** | One of &#x60;\&quot;not-running\&quot;&#x60; (default), &#x60;\&quot;next-exit\&quot;&#x60;, or &#x60;\&quot;removed\&quot;&#x60;. Matches Docker&#39;s &#x60;/containers/{id}/wait&#x60; semantics. Omitted values default to &#x60;\&quot;not-running\&quot;&#x60;. | 

### Return type

[**ContainerWaitDockerResponse**](ContainerWaitDockerResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)

