# \DaemonAPI

All URIs are relative to *http://localhost*

Method | HTTP request | Description
------------- | ------------- | -------------
[**GetDaemonCapabilities**](DaemonAPI.md#GetDaemonCapabilities) | **Get** /api/v1/daemon/capabilities | Get the daemon&#39;s runtime capability survey.



## GetDaemonCapabilities

> DaemonCapabilitiesResponse GetDaemonCapabilities(ctx).Execute()

Get the daemon's runtime capability survey.



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
	resp, r, err := apiClient.DaemonAPI.GetDaemonCapabilities(context.Background()).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `DaemonAPI.GetDaemonCapabilities``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `GetDaemonCapabilities`: DaemonCapabilitiesResponse
	fmt.Fprintf(os.Stdout, "Response from `DaemonAPI.GetDaemonCapabilities`: %v\n", resp)
}
```

### Path Parameters

This endpoint does not need any parameter.

### Other Parameters

Other parameters are passed through a pointer to a apiGetDaemonCapabilitiesRequest struct via the builder pattern


### Return type

[**DaemonCapabilitiesResponse**](DaemonCapabilitiesResponse.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)

