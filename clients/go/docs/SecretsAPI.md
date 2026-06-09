# \SecretsAPI

All URIs are relative to *http://localhost*

Method | HTTP request | Description
------------- | ------------- | -------------
[**BulkImportSecrets**](SecretsAPI.md#BulkImportSecrets) | **Post** /api/v1/secrets/bulk-import | Bulk-import secrets from a dotenv-style payload (&#x60;KEY&#x3D;value\\n…&#x60;).
[**CreateSecret**](SecretsAPI.md#CreateSecret) | **Post** /api/v1/secrets | Create or update a secret.
[**DeleteSecret**](SecretsAPI.md#DeleteSecret) | **Delete** /api/v1/secrets/{name} | Delete a secret.
[**GetSecretMetadata**](SecretsAPI.md#GetSecretMetadata) | **Get** /api/v1/secrets/{name} | Get metadata for a specific secret. With &#x60;?reveal&#x3D;true&#x60; (admin only), the response also includes the plaintext &#x60;value&#x60;.
[**ListSecrets**](SecretsAPI.md#ListSecrets) | **Get** /api/v1/secrets | List secrets in a scope.
[**RevealAllSecrets**](SecretsAPI.md#RevealAllSecrets) | **Get** /api/v1/secrets/reveal-all | Reveal every secret in an environment at once (admin only).
[**RotateSecret**](SecretsAPI.md#RotateSecret) | **Post** /api/v1/secrets/{name}/rotate | Rotate a secret — overwrite with a new value and return the version before+after.



## BulkImportSecrets

> BulkImportResponse BulkImportSecrets(ctx).Environment(environment).Body(body).Execute()

Bulk-import secrets from a dotenv-style payload (`KEY=value\\n…`).



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
	environment := "environment_example" // string | Environment id to import into
	body := "body_example" // string | 

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.SecretsAPI.BulkImportSecrets(context.Background()).Environment(environment).Body(body).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `SecretsAPI.BulkImportSecrets``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `BulkImportSecrets`: BulkImportResponse
	fmt.Fprintf(os.Stdout, "Response from `SecretsAPI.BulkImportSecrets`: %v\n", resp)
}
```

### Path Parameters



### Other Parameters

Other parameters are passed through a pointer to a apiBulkImportSecretsRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
 **environment** | **string** | Environment id to import into | 
 **body** | **string** |  | 

### Return type

[**BulkImportResponse**](BulkImportResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: text/plain
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## CreateSecret

> SecretMetadataResponse CreateSecret(ctx).CreateSecretRequest(createSecretRequest).Environment(environment).Scope(scope).Execute()

Create or update a secret.



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
	createSecretRequest := *openapiclient.NewCreateSecretRequest("Name_example", "Value_example") // CreateSecretRequest | 
	environment := "environment_example" // string | Environment id (mutually exclusive with body 'scope') (optional)
	scope := "scope_example" // string | Explicit scope (legacy) (optional)

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.SecretsAPI.CreateSecret(context.Background()).CreateSecretRequest(createSecretRequest).Environment(environment).Scope(scope).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `SecretsAPI.CreateSecret``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `CreateSecret`: SecretMetadataResponse
	fmt.Fprintf(os.Stdout, "Response from `SecretsAPI.CreateSecret`: %v\n", resp)
}
```

### Path Parameters



### Other Parameters

Other parameters are passed through a pointer to a apiCreateSecretRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
 **createSecretRequest** | [**CreateSecretRequest**](CreateSecretRequest.md) |  | 
 **environment** | **string** | Environment id (mutually exclusive with body &#39;scope&#39;) | 
 **scope** | **string** | Explicit scope (legacy) | 

### Return type

[**SecretMetadataResponse**](SecretMetadataResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: application/json
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## DeleteSecret

> DeleteSecret(ctx, name).Environment(environment).Scope(scope).Execute()

Delete a secret.



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
	name := "name_example" // string | Secret name
	environment := "environment_example" // string | Environment id (optional)
	scope := "scope_example" // string | Explicit scope (optional)

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	r, err := apiClient.SecretsAPI.DeleteSecret(context.Background(), name).Environment(environment).Scope(scope).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `SecretsAPI.DeleteSecret``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**name** | **string** | Secret name | 

### Other Parameters

Other parameters are passed through a pointer to a apiDeleteSecretRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------

 **environment** | **string** | Environment id | 
 **scope** | **string** | Explicit scope | 

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


## GetSecretMetadata

> SecretMetadataResponse GetSecretMetadata(ctx, name).Environment(environment).Scope(scope).Reveal(reveal).Execute()

Get metadata for a specific secret. With `?reveal=true` (admin only), the response also includes the plaintext `value`.



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
	name := "name_example" // string | Secret name
	environment := "environment_example" // string | Environment id (optional)
	scope := "scope_example" // string | Explicit scope (optional)
	reveal := true // bool | Include plaintext value (admin only) (optional)

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.SecretsAPI.GetSecretMetadata(context.Background(), name).Environment(environment).Scope(scope).Reveal(reveal).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `SecretsAPI.GetSecretMetadata``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `GetSecretMetadata`: SecretMetadataResponse
	fmt.Fprintf(os.Stdout, "Response from `SecretsAPI.GetSecretMetadata`: %v\n", resp)
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**name** | **string** | Secret name | 

### Other Parameters

Other parameters are passed through a pointer to a apiGetSecretMetadataRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------

 **environment** | **string** | Environment id | 
 **scope** | **string** | Explicit scope | 
 **reveal** | **bool** | Include plaintext value (admin only) | 

### Return type

[**SecretMetadataResponse**](SecretMetadataResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## ListSecrets

> []SecretMetadataResponse ListSecrets(ctx).Environment(environment).Scope(scope).Execute()

List secrets in a scope.



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
	environment := "environment_example" // string | Environment id (optional)
	scope := "scope_example" // string | Explicit scope (optional)

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.SecretsAPI.ListSecrets(context.Background()).Environment(environment).Scope(scope).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `SecretsAPI.ListSecrets``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `ListSecrets`: []SecretMetadataResponse
	fmt.Fprintf(os.Stdout, "Response from `SecretsAPI.ListSecrets`: %v\n", resp)
}
```

### Path Parameters



### Other Parameters

Other parameters are passed through a pointer to a apiListSecretsRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
 **environment** | **string** | Environment id | 
 **scope** | **string** | Explicit scope | 

### Return type

[**[]SecretMetadataResponse**](SecretMetadataResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## RevealAllSecrets

> RevealAllSecretsResponse RevealAllSecrets(ctx).Environment(environment).Execute()

Reveal every secret in an environment at once (admin only).



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
	environment := "environment_example" // string | Environment id (required)

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.SecretsAPI.RevealAllSecrets(context.Background()).Environment(environment).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `SecretsAPI.RevealAllSecrets``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `RevealAllSecrets`: RevealAllSecretsResponse
	fmt.Fprintf(os.Stdout, "Response from `SecretsAPI.RevealAllSecrets`: %v\n", resp)
}
```

### Path Parameters



### Other Parameters

Other parameters are passed through a pointer to a apiRevealAllSecretsRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
 **environment** | **string** | Environment id (required) | 

### Return type

[**RevealAllSecretsResponse**](RevealAllSecretsResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)


## RotateSecret

> RotateSecretResponse RotateSecret(ctx, name).RotateSecretRequest(rotateSecretRequest).Environment(environment).Scope(scope).Execute()

Rotate a secret — overwrite with a new value and return the version before+after.



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
	name := "name_example" // string | Secret name
	rotateSecretRequest := *openapiclient.NewRotateSecretRequest("Value_example") // RotateSecretRequest | 
	environment := "environment_example" // string | Environment id (optional)
	scope := "scope_example" // string | Explicit scope (legacy) (optional)

	configuration := openapiclient.NewConfiguration()
	apiClient := openapiclient.NewAPIClient(configuration)
	resp, r, err := apiClient.SecretsAPI.RotateSecret(context.Background(), name).RotateSecretRequest(rotateSecretRequest).Environment(environment).Scope(scope).Execute()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error when calling `SecretsAPI.RotateSecret``: %v\n", err)
		fmt.Fprintf(os.Stderr, "Full HTTP response: %v\n", r)
	}
	// response from `RotateSecret`: RotateSecretResponse
	fmt.Fprintf(os.Stdout, "Response from `SecretsAPI.RotateSecret`: %v\n", resp)
}
```

### Path Parameters


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------
**ctx** | **context.Context** | context for authentication, logging, cancellation, deadlines, tracing, etc.
**name** | **string** | Secret name | 

### Other Parameters

Other parameters are passed through a pointer to a apiRotateSecretRequest struct via the builder pattern


Name | Type | Description  | Notes
------------- | ------------- | ------------- | -------------

 **rotateSecretRequest** | [**RotateSecretRequest**](RotateSecretRequest.md) |  | 
 **environment** | **string** | Environment id | 
 **scope** | **string** | Explicit scope (legacy) | 

### Return type

[**RotateSecretResponse**](RotateSecretResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: application/json
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints)
[[Back to Model list]](../README.md#documentation-for-models)
[[Back to README]](../README.md)

