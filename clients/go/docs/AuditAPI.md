# \AuditAPI

All URIs are relative to *http://localhost*

Method | HTTP request | Description
------------- | ------------- | -------------
[**ListAudit**](AuditAPI.md#ListAudit) | **Get** /api/v1/audit | List audit log entries. Admin only.



## ListAudit

> []AuditEntry ListAudit(ctx).User(user).ResourceKind(resourceKind).Since(since).Until(until).Limit(limit).Execute()

List audit log entries. Admin only.



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
	user := "user_example" // string | Filter by user id. (optional)
	resourceKind := "resourceKind_example" // string | Filter by resource kind. (optional)
	since := "since_example" // string | Only entries at or after this timestamp (RFC 3339). (optional)
	until := "until_example" // string | Only entries at or before this timestamp (RFC 3339). (optional)
	limit := int32(56) // int32 | Maximum number of entries to return (default 100). (optional)

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.AuditAPI.ListAudit(context.Background()).User(user).ResourceKind(resourceKind).Since(since).Until(until).Limit(limit).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `AuditAPI.ListAudit``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `ListAudit`: []AuditEntry
	fmt.Fprintf(os.Stdout, "Response from `AuditAPI.ListAudit`: %v\n", resp)
}
```

### Path Parameters



### Other Parameters

Other parameters are passed through a pointer to a apiListAuditRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
 **user** | **string** | Filter by user id. | 
 **resourceKind** | **string** | Filter by resource kind. | 
 **since** | **string** | Only entries at or after this timestamp (RFC 3339). | 
 **until** | **string** | Only entries at or before this timestamp (RFC 3339). | 
 **limit** | **int32** | Maximum number of entries to return (default 100). | 

### Return type

[**[]AuditEntry**](AuditEntry.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)

