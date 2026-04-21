# \NotifiersAPI

All URIs are relative to *http://localhost*

Method | HTTP request | Description
------------- | ------------- | -------------
[**CreateNotifier**](NotifiersAPI.md#CreateNotifier) | **Post** /api/v1/notifiers | Create a new notifier. Admin only.
[**DeleteNotifier**](NotifiersAPI.md#DeleteNotifier) | **Delete** /api/v1/notifiers/{id} | Delete a notifier. Admin only.
[**GetNotifier**](NotifiersAPI.md#GetNotifier) | **Get** /api/v1/notifiers/{id} | Fetch a single notifier by id.
[**ListNotifiers**](NotifiersAPI.md#ListNotifiers) | **Get** /api/v1/notifiers | List notifiers.
[**TestNotifier**](NotifiersAPI.md#TestNotifier) | **Post** /api/v1/notifiers/{id}/test | Send a test notification through a notifier. Admin only.
[**UpdateNotifier**](NotifiersAPI.md#UpdateNotifier) | **Patch** /api/v1/notifiers/{id} | Update a notifier. Admin only.



## CreateNotifier

> StoredNotifier CreateNotifier(ctx).CreateNotifierRequest(createNotifierRequest).Execute()

Create a new notifier. Admin only.



### Example

```go
package main

import (
	"context"
	"fmt"
	"os"
	openapiclient "github.com/GIT_USER_ID/GIT_REPO_ID"
)

func main() {
	createNotifierRequest := *openapiclient.NewCreateNotifierRequest(openapiclient.NotifierConfig{NotifierConfigOneOf: openapiclient.NewNotifierConfigOneOf("Type_example", "WebhookUrl_example")}, openapiclient.NotifierKind("slack"), "Name_example") // CreateNotifierRequest | 

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.NotifiersAPI.CreateNotifier(context.Background()).CreateNotifierRequest(createNotifierRequest).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `NotifiersAPI.CreateNotifier``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `CreateNotifier`: StoredNotifier
	fmt.Fprintf(os.Stdout, "Response from `NotifiersAPI.CreateNotifier`: %v\n", resp)
}
```

### Path Parameters



### Other Parameters

Other parameters are passed through a pointer to a apiCreateNotifierRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
 **createNotifierRequest** | [**CreateNotifierRequest**](CreateNotifierRequest.md) |  | 

### Return type

[**StoredNotifier**](StoredNotifier.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: application/json
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## DeleteNotifier

> DeleteNotifier(ctx, id).Execute()

Delete a notifier. Admin only.



### Example

```go
package main

import (
	"context"
	"fmt"
	"os"
	openapiclient "github.com/GIT_USER_ID/GIT_REPO_ID"
)

func main() {
	id := "id_example" // string | Notifier id

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	r, err := apiClient.NotifiersAPI.DeleteNotifier(context.Background(), id).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `NotifiersAPI.DeleteNotifier``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**id** | **string** | Notifier id | 

### Other Parameters

Other parameters are passed through a pointer to a apiDeleteNotifierRequest struct via the builder pattern


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


## GetNotifier

> StoredNotifier GetNotifier(ctx, id).Execute()

Fetch a single notifier by id.



### Example

```go
package main

import (
	"context"
	"fmt"
	"os"
	openapiclient "github.com/GIT_USER_ID/GIT_REPO_ID"
)

func main() {
	id := "id_example" // string | Notifier id

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.NotifiersAPI.GetNotifier(context.Background(), id).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `NotifiersAPI.GetNotifier``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `GetNotifier`: StoredNotifier
	fmt.Fprintf(os.Stdout, "Response from `NotifiersAPI.GetNotifier`: %v\n", resp)
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**id** | **string** | Notifier id | 

### Other Parameters

Other parameters are passed through a pointer to a apiGetNotifierRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------


### Return type

[**StoredNotifier**](StoredNotifier.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## ListNotifiers

> []StoredNotifier ListNotifiers(ctx).Execute()

List notifiers.



### Example

```go
package main

import (
	"context"
	"fmt"
	"os"
	openapiclient "github.com/GIT_USER_ID/GIT_REPO_ID"
)

func main() {

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.NotifiersAPI.ListNotifiers(context.Background()).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `NotifiersAPI.ListNotifiers``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `ListNotifiers`: []StoredNotifier
	fmt.Fprintf(os.Stdout, "Response from `NotifiersAPI.ListNotifiers`: %v\n", resp)
}
```

### Path Parameters

This endpoint does not need any parameter.

### Other Parameters

Other parameters are passed through a pointer to a apiListNotifiersRequest struct via the builder pattern


### Return type

[**[]StoredNotifier**](StoredNotifier.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## TestNotifier

> TestNotifierResponse TestNotifier(ctx, id).Execute()

Send a test notification through a notifier. Admin only.



### Example

```go
package main

import (
	"context"
	"fmt"
	"os"
	openapiclient "github.com/GIT_USER_ID/GIT_REPO_ID"
)

func main() {
	id := "id_example" // string | Notifier id

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.NotifiersAPI.TestNotifier(context.Background(), id).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `NotifiersAPI.TestNotifier``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `TestNotifier`: TestNotifierResponse
	fmt.Fprintf(os.Stdout, "Response from `NotifiersAPI.TestNotifier`: %v\n", resp)
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**id** | **string** | Notifier id | 

### Other Parameters

Other parameters are passed through a pointer to a apiTestNotifierRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------


### Return type

[**TestNotifierResponse**](TestNotifierResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## UpdateNotifier

> StoredNotifier UpdateNotifier(ctx, id).UpdateNotifierRequest(updateNotifierRequest).Execute()

Update a notifier. Admin only.



### Example

```go
package main

import (
	"context"
	"fmt"
	"os"
	openapiclient "github.com/GIT_USER_ID/GIT_REPO_ID"
)

func main() {
	id := "id_example" // string | Notifier id
	updateNotifierRequest := *openapiclient.NewUpdateNotifierRequest() // UpdateNotifierRequest | 

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.NotifiersAPI.UpdateNotifier(context.Background(), id).UpdateNotifierRequest(updateNotifierRequest).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `NotifiersAPI.UpdateNotifier``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `UpdateNotifier`: StoredNotifier
	fmt.Fprintf(os.Stdout, "Response from `NotifiersAPI.UpdateNotifier`: %v\n", resp)
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**id** | **string** | Notifier id | 

### Other Parameters

Other parameters are passed through a pointer to a apiUpdateNotifierRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------

 **updateNotifierRequest** | [**UpdateNotifierRequest**](UpdateNotifierRequest.md) |  | 

### Return type

[**StoredNotifier**](StoredNotifier.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: application/json
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)

