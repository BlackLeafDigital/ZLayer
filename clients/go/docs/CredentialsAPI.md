# \CredentialsAPI

All URIs are relative to *http://localhost*

Method | HTTP request | Description
------------- | ------------- | -------------
[**CreateGitCredential**](CredentialsAPI.md#CreateGitCredential) | **Post** /api/v1/credentials/git | Create a new git credential. Admin only.
[**CreateRegistryCredential**](CredentialsAPI.md#CreateRegistryCredential) | **Post** /api/v1/credentials/registry | Create a new registry credential. Admin only.
[**DeleteGitCredential**](CredentialsAPI.md#DeleteGitCredential) | **Delete** /api/v1/credentials/git/{id} | Delete a git credential. Admin only.
[**DeleteRegistryCredential**](CredentialsAPI.md#DeleteRegistryCredential) | **Delete** /api/v1/credentials/registry/{id} | Delete a registry credential. Admin only.
[**ListGitCredentials**](CredentialsAPI.md#ListGitCredentials) | **Get** /api/v1/credentials/git | List all git credentials (metadata only, no secret values).
[**ListRegistryCredentials**](CredentialsAPI.md#ListRegistryCredentials) | **Get** /api/v1/credentials/registry | List all registry credentials (metadata only, no passwords).



## CreateGitCredential

> GitCredentialResponse CreateGitCredential(ctx).CreateGitCredentialRequest(createGitCredentialRequest).Execute()

Create a new git credential. Admin only.



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
	createGitCredentialRequest := *openapiclient.NewCreateGitCredentialRequest(openapiclient.GitCredentialKindSchema("pat"), "Name_example", "Value_example") // CreateGitCredentialRequest | 

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.CredentialsAPI.CreateGitCredential(context.Background()).CreateGitCredentialRequest(createGitCredentialRequest).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `CredentialsAPI.CreateGitCredential``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `CreateGitCredential`: GitCredentialResponse
	fmt.Fprintf(os.Stdout, "Response from `CredentialsAPI.CreateGitCredential`: %v\n", resp)
}
```

### Path Parameters



### Other Parameters

Other parameters are passed through a pointer to a apiCreateGitCredentialRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
 **createGitCredentialRequest** | [**CreateGitCredentialRequest**](CreateGitCredentialRequest.md) |  | 

### Return type

[**GitCredentialResponse**](GitCredentialResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: application/json
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## CreateRegistryCredential

> RegistryCredentialResponse CreateRegistryCredential(ctx).CreateRegistryCredentialRequest(createRegistryCredentialRequest).Execute()

Create a new registry credential. Admin only.



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
	createRegistryCredentialRequest := *openapiclient.NewCreateRegistryCredentialRequest(openapiclient.RegistryAuthTypeSchema("basic"), "Password_example", "Registry_example", "Username_example") // CreateRegistryCredentialRequest | 

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.CredentialsAPI.CreateRegistryCredential(context.Background()).CreateRegistryCredentialRequest(createRegistryCredentialRequest).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `CredentialsAPI.CreateRegistryCredential``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `CreateRegistryCredential`: RegistryCredentialResponse
	fmt.Fprintf(os.Stdout, "Response from `CredentialsAPI.CreateRegistryCredential`: %v\n", resp)
}
```

### Path Parameters



### Other Parameters

Other parameters are passed through a pointer to a apiCreateRegistryCredentialRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
 **createRegistryCredentialRequest** | [**CreateRegistryCredentialRequest**](CreateRegistryCredentialRequest.md) |  | 

### Return type

[**RegistryCredentialResponse**](RegistryCredentialResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: application/json
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## DeleteGitCredential

> DeleteGitCredential(ctx, id).Execute()

Delete a git credential. Admin only.



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
	id := "id_example" // string | Git credential id

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	r, err := apiClient.CredentialsAPI.DeleteGitCredential(context.Background(), id).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `CredentialsAPI.DeleteGitCredential``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**id** | **string** | Git credential id | 

### Other Parameters

Other parameters are passed through a pointer to a apiDeleteGitCredentialRequest struct via the builder pattern


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


## DeleteRegistryCredential

> DeleteRegistryCredential(ctx, id).Execute()

Delete a registry credential. Admin only.



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
	id := "id_example" // string | Registry credential id

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	r, err := apiClient.CredentialsAPI.DeleteRegistryCredential(context.Background(), id).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `CredentialsAPI.DeleteRegistryCredential``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**id** | **string** | Registry credential id | 

### Other Parameters

Other parameters are passed through a pointer to a apiDeleteRegistryCredentialRequest struct via the builder pattern


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


## ListGitCredentials

> []GitCredentialResponse ListGitCredentials(ctx).Execute()

List all git credentials (metadata only, no secret values).



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
	resp, r, err := apiClient.CredentialsAPI.ListGitCredentials(context.Background()).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `CredentialsAPI.ListGitCredentials``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `ListGitCredentials`: []GitCredentialResponse
	fmt.Fprintf(os.Stdout, "Response from `CredentialsAPI.ListGitCredentials`: %v\n", resp)
}
```

### Path Parameters

This endpoint does not need any parameter.

### Other Parameters

Other parameters are passed through a pointer to a apiListGitCredentialsRequest struct via the builder pattern


### Return type

[**[]GitCredentialResponse**](GitCredentialResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## ListRegistryCredentials

> []RegistryCredentialResponse ListRegistryCredentials(ctx).Execute()

List all registry credentials (metadata only, no passwords).



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
	resp, r, err := apiClient.CredentialsAPI.ListRegistryCredentials(context.Background()).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `CredentialsAPI.ListRegistryCredentials``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `ListRegistryCredentials`: []RegistryCredentialResponse
	fmt.Fprintf(os.Stdout, "Response from `CredentialsAPI.ListRegistryCredentials`: %v\n", resp)
}
```

### Path Parameters

This endpoint does not need any parameter.

### Other Parameters

Other parameters are passed through a pointer to a apiListRegistryCredentialsRequest struct via the builder pattern


### Return type

[**[]RegistryCredentialResponse**](RegistryCredentialResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)

