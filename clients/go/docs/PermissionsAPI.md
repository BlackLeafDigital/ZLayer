# \PermissionsAPI

All URIs are relative to *http://localhost*

Method | HTTP request | Description
------------- | ------------- | -------------
[**GrantPermission**](PermissionsAPI.md#GrantPermission) | **Post** /api/v1/permissions | Grant a permission. Admin only.
[**ListPermissions**](PermissionsAPI.md#ListPermissions) | **Get** /api/v1/permissions | List permissions for a subject (user or group).
[**ListPermissionsByResource**](PermissionsAPI.md#ListPermissionsByResource) | **Get** /api/v1/permissions/by-resource | List permissions granted on a specific resource.
[**RevokePermission**](PermissionsAPI.md#RevokePermission) | **Delete** /api/v1/permissions/{id} | Revoke a permission by id. Admin only.



## GrantPermission

> StoredPermission GrantPermission(ctx).GrantPermissionRequest(grantPermissionRequest).Execute()

Grant a permission. Admin only.



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
	grantPermissionRequest := *openapiclient.NewGrantPermissionRequest(openapiclient.PermissionLevel("none"), "ResourceKind_example", "SubjectId_example", openapiclient.SubjectKind("user")) // GrantPermissionRequest | 

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.PermissionsAPI.GrantPermission(context.Background()).GrantPermissionRequest(grantPermissionRequest).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `PermissionsAPI.GrantPermission``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `GrantPermission`: StoredPermission
	fmt.Fprintf(os.Stdout, "Response from `PermissionsAPI.GrantPermission`: %v\n", resp)
}
```

### Path Parameters



### Other Parameters

Other parameters are passed through a pointer to a apiGrantPermissionRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
 **grantPermissionRequest** | [**GrantPermissionRequest**](GrantPermissionRequest.md) |  | 

### Return type

[**StoredPermission**](StoredPermission.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: application/json
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## ListPermissions

> []StoredPermission ListPermissions(ctx).User(user).Group(group).Execute()

List permissions for a subject (user or group).



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
	user := "user_example" // string | Filter by user id. (optional)
	group := "group_example" // string | Filter by group id. (optional)

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.PermissionsAPI.ListPermissions(context.Background()).User(user).Group(group).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `PermissionsAPI.ListPermissions``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `ListPermissions`: []StoredPermission
	fmt.Fprintf(os.Stdout, "Response from `PermissionsAPI.ListPermissions`: %v\n", resp)
}
```

### Path Parameters



### Other Parameters

Other parameters are passed through a pointer to a apiListPermissionsRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
 **user** | **string** | Filter by user id. | 
 **group** | **string** | Filter by group id. | 

### Return type

[**[]StoredPermission**](StoredPermission.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## ListPermissionsByResource

> []StoredPermission ListPermissionsByResource(ctx).Kind(kind).Id(id).Execute()

List permissions granted on a specific resource.



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
	kind := "kind_example" // string | Resource kind (e.g. `\"environment\"`).
	id := "id_example" // string | Specific resource id. Omit for wildcard grants only. (optional)

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.PermissionsAPI.ListPermissionsByResource(context.Background()).Kind(kind).Id(id).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `PermissionsAPI.ListPermissionsByResource``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `ListPermissionsByResource`: []StoredPermission
	fmt.Fprintf(os.Stdout, "Response from `PermissionsAPI.ListPermissionsByResource`: %v\n", resp)
}
```

### Path Parameters



### Other Parameters

Other parameters are passed through a pointer to a apiListPermissionsByResourceRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
 **kind** | **string** | Resource kind (e.g. &#x60;\&quot;environment\&quot;&#x60;). | 
 **id** | **string** | Specific resource id. Omit for wildcard grants only. | 

### Return type

[**[]StoredPermission**](StoredPermission.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## RevokePermission

> RevokePermission(ctx, id).Execute()

Revoke a permission by id. Admin only.



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
	id := "id_example" // string | Permission id

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	r, err := apiClient.PermissionsAPI.RevokePermission(context.Background(), id).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `PermissionsAPI.RevokePermission``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**id** | **string** | Permission id | 

### Other Parameters

Other parameters are passed through a pointer to a apiRevokePermissionRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------


### Return type

 (empty response body)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: Not defined

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)

