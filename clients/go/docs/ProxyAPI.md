# \ProxyAPI

All URIs are relative to *http://localhost*

Method | HTTP request | Description
------------- | ------------- | -------------
[**ListBackends**](ProxyAPI.md#ListBackends) | **Get** /api/v1/proxy/backends | List all load-balancer backend groups.
[**ListRoutes**](ProxyAPI.md#ListRoutes) | **Get** /api/v1/proxy/routes | List all registered proxy routes.
[**ListStreams**](ProxyAPI.md#ListStreams) | **Get** /api/v1/proxy/streams | List L4 stream proxies.
[**ListTls**](ProxyAPI.md#ListTls) | **Get** /api/v1/proxy/tls | List loaded TLS certificates.



## ListBackends

> BackendsResponse ListBackends(ctx).Execute()

List all load-balancer backend groups.



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
	resp, r, err := apiClient.ProxyAPI.ListBackends(context.Background()).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `ProxyAPI.ListBackends``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `ListBackends`: BackendsResponse
	fmt.Fprintf(os.Stdout, "Response from `ProxyAPI.ListBackends`: %v\n", resp)
}
```

### Path Parameters

This endpoint does not need any parameter.

### Other Parameters

Other parameters are passed through a pointer to a apiListBackendsRequest struct via the builder pattern


### Return type

[**BackendsResponse**](BackendsResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## ListRoutes

> RoutesResponse ListRoutes(ctx).Execute()

List all registered proxy routes.



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
	resp, r, err := apiClient.ProxyAPI.ListRoutes(context.Background()).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `ProxyAPI.ListRoutes``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `ListRoutes`: RoutesResponse
	fmt.Fprintf(os.Stdout, "Response from `ProxyAPI.ListRoutes`: %v\n", resp)
}
```

### Path Parameters

This endpoint does not need any parameter.

### Other Parameters

Other parameters are passed through a pointer to a apiListRoutesRequest struct via the builder pattern


### Return type

[**RoutesResponse**](RoutesResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## ListStreams

> StreamsResponse ListStreams(ctx).Execute()

List L4 stream proxies.



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
	resp, r, err := apiClient.ProxyAPI.ListStreams(context.Background()).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `ProxyAPI.ListStreams``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `ListStreams`: StreamsResponse
	fmt.Fprintf(os.Stdout, "Response from `ProxyAPI.ListStreams`: %v\n", resp)
}
```

### Path Parameters

This endpoint does not need any parameter.

### Other Parameters

Other parameters are passed through a pointer to a apiListStreamsRequest struct via the builder pattern


### Return type

[**StreamsResponse**](StreamsResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## ListTls

> TlsResponse ListTls(ctx).Execute()

List loaded TLS certificates.



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
	resp, r, err := apiClient.ProxyAPI.ListTls(context.Background()).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `ProxyAPI.ListTls``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `ListTls`: TlsResponse
	fmt.Fprintf(os.Stdout, "Response from `ProxyAPI.ListTls`: %v\n", resp)
}
```

### Path Parameters

This endpoint does not need any parameter.

### Other Parameters

Other parameters are passed through a pointer to a apiListTlsRequest struct via the builder pattern


### Return type

[**TlsResponse**](TlsResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)

