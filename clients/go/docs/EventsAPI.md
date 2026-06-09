# \EventsAPI

All URIs are relative to *http://localhost*

Method | HTTP request | Description
------------- | ------------- | -------------
[**StreamEvents**](EventsAPI.md#StreamEvents) | **Get** /api/v1/events | Stream daemon lifecycle events as NDJSON.



## StreamEvents

> StreamEvents(ctx).Follow(follow).Label(label).Execute()

Stream daemon lifecycle events as NDJSON.



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
	follow := true // bool | Follow the event stream. Default: `true`. Reserved for parity with Docker-compat tooling; this endpoint is always streaming. (optional)
	label := []string{"Inner_example"} // []string | Label filter in `k=v` form. Repeatable. An event passes only if all filters match (AND semantics). (optional)

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	r, err := apiClient.EventsAPI.StreamEvents(context.Background()).Follow(follow).Label(label).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `EventsAPI.StreamEvents``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
}
```

### Path Parameters



### Other Parameters

Other parameters are passed through a pointer to a apiStreamEventsRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
 **follow** | **bool** | Follow the event stream. Default: &#x60;true&#x60;. Reserved for parity with Docker-compat tooling; this endpoint is always streaming. | 
 **label** | **[]string** | Label filter in &#x60;k&#x3D;v&#x60; form. Repeatable. An event passes only if all filters match (AND semantics). | 

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

