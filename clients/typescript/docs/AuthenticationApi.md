# AuthenticationApi

All URIs are relative to *http://localhost*

| Method | HTTP request | Description |
|------------- | ------------- | -------------|
| [**bootstrap**](AuthenticationApi.md#bootstrapoperation) | **POST** /auth/bootstrap | Bootstrap the very first admin user. Returns 409 if any user exists. |
| [**callback**](AuthenticationApi.md#callback) | **GET** /auth/oidc/{provider}/callback | &#x60;GET /auth/oidc/:provider/callback&#x60;. |
| [**csrf**](AuthenticationApi.md#csrf) | **GET** /auth/csrf | Rotate the CSRF double-submit token for the current session. |
| [**getToken**](AuthenticationApi.md#gettoken) | **POST** /auth/token | Get an access token. |
| [**listProviders**](AuthenticationApi.md#listproviders) | **GET** /auth/oidc/providers | &#x60;GET /auth/oidc/providers&#x60;. |
| [**login**](AuthenticationApi.md#loginoperation) | **POST** /auth/login | Sign in an existing user. |
| [**logout**](AuthenticationApi.md#logout) | **POST** /auth/logout | Clear the session + CSRF cookies. |
| [**me**](AuthenticationApi.md#me) | **GET** /auth/me | Return the currently signed-in user. |
| [**start**](AuthenticationApi.md#start) | **GET** /auth/oidc/{provider}/start | &#x60;GET /auth/oidc/:provider/start&#x60; — 302 to the provider\&#39;s authorize URL. |



## bootstrap

> LoginResponse bootstrap(bootstrapRequest)

Bootstrap the very first admin user. Returns 409 if any user exists.

# Errors  Returns an error if the user store or credential store is not configured, if any user already exists, if the request is missing required fields, or if the underlying stores fail.

### Example

```ts
import {
  Configuration,
  AuthenticationApi,
} from '@zlayer/api-client';
import type { BootstrapOperationRequest } from '@zlayer/api-client';

async function example() {
  console.log("🚀 Testing @zlayer/api-client SDK...");
  const api = new AuthenticationApi();

  const body = {
    // BootstrapRequest
    bootstrapRequest: ...,
  } satisfies BootstrapOperationRequest;

  try {
    const data = await api.bootstrap(body);
    console.log(data);
  } catch (error) {
    console.error(error);
  }
}

// Run the test
example().catch(console.error);
```

### Parameters


| Name | Type | Description  | Notes |
|------------- | ------------- | ------------- | -------------|
| **bootstrapRequest** | [BootstrapRequest](BootstrapRequest.md) |  | |

### Return type

[**LoginResponse**](LoginResponse.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: `application/json`
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **201** | Admin user created |  -  |
| **400** | Invalid email or password |  -  |
| **409** | Bootstrap already completed |  -  |
| **503** | User store not configured |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## callback

> OidcCallbackResponse callback(provider)

&#x60;GET /auth/oidc/:provider/callback&#x60;.

### Example

```ts
import {
  Configuration,
  AuthenticationApi,
} from '@zlayer/api-client';
import type { CallbackRequest } from '@zlayer/api-client';

async function example() {
  console.log("🚀 Testing @zlayer/api-client SDK...");
  const api = new AuthenticationApi();

  const body = {
    // string | Provider slug
    provider: provider_example,
  } satisfies CallbackRequest;

  try {
    const data = await api.callback(body);
    console.log(data);
  } catch (error) {
    console.error(error);
  }
}

// Run the test
example().catch(console.error);
```

### Parameters


| Name | Type | Description  | Notes |
|------------- | ------------- | ------------- | -------------|
| **provider** | `string` | Provider slug | [Defaults to `undefined`] |

### Return type

[**OidcCallbackResponse**](OidcCallbackResponse.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Signed in via OIDC |  -  |
| **400** | Invalid callback parameters |  -  |
| **401** | Provider returned an error |  -  |
| **404** | Unknown provider |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## csrf

> CsrfResponse csrf()

Rotate the CSRF double-submit token for the current session.

### Example

```ts
import {
  Configuration,
  AuthenticationApi,
} from '@zlayer/api-client';
import type { CsrfRequest } from '@zlayer/api-client';

async function example() {
  console.log("🚀 Testing @zlayer/api-client SDK...");
  const api = new AuthenticationApi();

  try {
    const data = await api.csrf();
    console.log(data);
  } catch (error) {
    console.error(error);
  }
}

// Run the test
example().catch(console.error);
```

### Parameters

This endpoint does not need any parameter.

### Return type

[**CsrfResponse**](CsrfResponse.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Rotated CSRF token |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## getToken

> TokenResponse getToken(tokenRequest)

Get an access token.

# Errors  Returns an error if credentials are invalid or token creation fails.

### Example

```ts
import {
  Configuration,
  AuthenticationApi,
} from '@zlayer/api-client';
import type { GetTokenRequest } from '@zlayer/api-client';

async function example() {
  console.log("🚀 Testing @zlayer/api-client SDK...");
  const api = new AuthenticationApi();

  const body = {
    // TokenRequest
    tokenRequest: ...,
  } satisfies GetTokenRequest;

  try {
    const data = await api.getToken(body);
    console.log(data);
  } catch (error) {
    console.error(error);
  }
}

// Run the test
example().catch(console.error);
```

### Parameters


| Name | Type | Description  | Notes |
|------------- | ------------- | ------------- | -------------|
| **tokenRequest** | [TokenRequest](TokenRequest.md) |  | |

### Return type

[**TokenResponse**](TokenResponse.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: `application/json`
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Token created |  -  |
| **401** | Invalid credentials |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## listProviders

> Array&lt;OidcProviderPublic&gt; listProviders()

&#x60;GET /auth/oidc/providers&#x60;.

### Example

```ts
import {
  Configuration,
  AuthenticationApi,
} from '@zlayer/api-client';
import type { ListProvidersRequest } from '@zlayer/api-client';

async function example() {
  console.log("🚀 Testing @zlayer/api-client SDK...");
  const api = new AuthenticationApi();

  try {
    const data = await api.listProviders();
    console.log(data);
  } catch (error) {
    console.error(error);
  }
}

// Run the test
example().catch(console.error);
```

### Parameters

This endpoint does not need any parameter.

### Return type

[**Array&lt;OidcProviderPublic&gt;**](OidcProviderPublic.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | List of configured OIDC providers |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## login

> LoginResponse login(loginRequest)

Sign in an existing user.

# Errors  Returns an error if credentials are invalid, the user is disabled, or any backing store fails.

### Example

```ts
import {
  Configuration,
  AuthenticationApi,
} from '@zlayer/api-client';
import type { LoginOperationRequest } from '@zlayer/api-client';

async function example() {
  console.log("🚀 Testing @zlayer/api-client SDK...");
  const api = new AuthenticationApi();

  const body = {
    // LoginRequest
    loginRequest: ...,
  } satisfies LoginOperationRequest;

  try {
    const data = await api.login(body);
    console.log(data);
  } catch (error) {
    console.error(error);
  }
}

// Run the test
example().catch(console.error);
```

### Parameters


| Name | Type | Description  | Notes |
|------------- | ------------- | ------------- | -------------|
| **loginRequest** | [LoginRequest](LoginRequest.md) |  | |

### Return type

[**LoginResponse**](LoginResponse.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: `application/json`
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Logged in |  -  |
| **401** | Invalid credentials |  -  |
| **403** | User disabled |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## logout

> logout()

Clear the session + CSRF cookies.

### Example

```ts
import {
  Configuration,
  AuthenticationApi,
} from '@zlayer/api-client';
import type { LogoutRequest } from '@zlayer/api-client';

async function example() {
  console.log("🚀 Testing @zlayer/api-client SDK...");
  const api = new AuthenticationApi();

  try {
    const data = await api.logout();
    console.log(data);
  } catch (error) {
    console.error(error);
  }
}

// Run the test
example().catch(console.error);
```

### Parameters

This endpoint does not need any parameter.

### Return type

`void` (Empty response body)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: Not defined


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **204** | Logged out |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## me

> UserView me()

Return the currently signed-in user.

# Errors  Returns an error if the user store is not configured, if the user no longer exists, or if the store fails.

### Example

```ts
import {
  Configuration,
  AuthenticationApi,
} from '@zlayer/api-client';
import type { MeRequest } from '@zlayer/api-client';

async function example() {
  console.log("🚀 Testing @zlayer/api-client SDK...");
  const api = new AuthenticationApi();

  try {
    const data = await api.me();
    console.log(data);
  } catch (error) {
    console.error(error);
  }
}

// Run the test
example().catch(console.error);
```

### Parameters

This endpoint does not need any parameter.

### Return type

[**UserView**](UserView.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Current user |  -  |
| **401** | Not signed in |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## start

> start(provider)

&#x60;GET /auth/oidc/:provider/start&#x60; — 302 to the provider\&#39;s authorize URL.

### Example

```ts
import {
  Configuration,
  AuthenticationApi,
} from '@zlayer/api-client';
import type { StartRequest } from '@zlayer/api-client';

async function example() {
  console.log("🚀 Testing @zlayer/api-client SDK...");
  const api = new AuthenticationApi();

  const body = {
    // string | Provider slug
    provider: provider_example,
  } satisfies StartRequest;

  try {
    const data = await api.start(body);
    console.log(data);
  } catch (error) {
    console.error(error);
  }
}

// Run the test
example().catch(console.error);
```

### Parameters


| Name | Type | Description  | Notes |
|------------- | ------------- | ------------- | -------------|
| **provider** | `string` | Provider slug | [Defaults to `undefined`] |

### Return type

`void` (Empty response body)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: Not defined


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **302** | Redirect to provider\&#39;s authorize URL |  -  |
| **404** | Unknown provider |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)

