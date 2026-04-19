# NotifiersApi

All URIs are relative to *http://localhost*

| Method | HTTP request | Description |
|------------- | ------------- | -------------|
| [**createNotifier**](NotifiersApi.md#createnotifieroperation) | **POST** /api/v1/notifiers | Create a new notifier. Admin only. |
| [**deleteNotifier**](NotifiersApi.md#deletenotifier) | **DELETE** /api/v1/notifiers/{id} | Delete a notifier. Admin only. |
| [**getNotifier**](NotifiersApi.md#getnotifier) | **GET** /api/v1/notifiers/{id} | Fetch a single notifier by id. |
| [**listNotifiers**](NotifiersApi.md#listnotifiers) | **GET** /api/v1/notifiers | List notifiers. |
| [**testNotifier**](NotifiersApi.md#testnotifier) | **POST** /api/v1/notifiers/{id}/test | Send a test notification through a notifier. Admin only. |
| [**updateNotifier**](NotifiersApi.md#updatenotifieroperation) | **PATCH** /api/v1/notifiers/{id} | Update a notifier. Admin only. |



## createNotifier

> StoredNotifier createNotifier(createNotifierRequest)

Create a new notifier. Admin only.

# Errors  Returns [&#x60;ApiError::Forbidden&#x60;] when the caller is not an admin, [&#x60;ApiError::BadRequest&#x60;] for an empty name or kind/config mismatch, or [&#x60;ApiError::Internal&#x60;] when the notifier store fails.

### Example

```ts
import {
  Configuration,
  NotifiersApi,
} from '@zlayer/client';
import type { CreateNotifierOperationRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new NotifiersApi(config);

  const body = {
    // CreateNotifierRequest
    createNotifierRequest: ...,
  } satisfies CreateNotifierOperationRequest;

  try {
    const data = await api.createNotifier(body);
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
| **createNotifierRequest** | [CreateNotifierRequest](CreateNotifierRequest.md) |  | |

### Return type

[**StoredNotifier**](StoredNotifier.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: `application/json`
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **201** | Notifier created |  -  |
| **400** | Invalid request |  -  |
| **403** | Admin role required |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## deleteNotifier

> deleteNotifier(id)

Delete a notifier. Admin only.

# Errors  Returns [&#x60;ApiError::Forbidden&#x60;] for non-admins, [&#x60;ApiError::NotFound&#x60;] when the notifier does not exist, or [&#x60;ApiError::Internal&#x60;] when the store fails.

### Example

```ts
import {
  Configuration,
  NotifiersApi,
} from '@zlayer/client';
import type { DeleteNotifierRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new NotifiersApi(config);

  const body = {
    // string | Notifier id
    id: id_example,
  } satisfies DeleteNotifierRequest;

  try {
    const data = await api.deleteNotifier(body);
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
| **id** | `string` | Notifier id | [Defaults to `undefined`] |

### Return type

`void` (Empty response body)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: Not defined


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **204** | Notifier deleted |  -  |
| **403** | Admin role required |  -  |
| **404** | Not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## getNotifier

> StoredNotifier getNotifier(id)

Fetch a single notifier by id.

# Errors  Returns [&#x60;ApiError::NotFound&#x60;] if no notifier with the given id exists, or [&#x60;ApiError::Internal&#x60;] when the store fails.

### Example

```ts
import {
  Configuration,
  NotifiersApi,
} from '@zlayer/client';
import type { GetNotifierRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new NotifiersApi(config);

  const body = {
    // string | Notifier id
    id: id_example,
  } satisfies GetNotifierRequest;

  try {
    const data = await api.getNotifier(body);
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
| **id** | `string` | Notifier id | [Defaults to `undefined`] |

### Return type

[**StoredNotifier**](StoredNotifier.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Notifier |  -  |
| **404** | Not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## listNotifiers

> Array&lt;StoredNotifier&gt; listNotifiers()

List notifiers.

# Errors  Returns [&#x60;ApiError::Internal&#x60;] if the notifier store fails.

### Example

```ts
import {
  Configuration,
  NotifiersApi,
} from '@zlayer/client';
import type { ListNotifiersRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new NotifiersApi(config);

  try {
    const data = await api.listNotifiers();
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

[**Array&lt;StoredNotifier&gt;**](StoredNotifier.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | List of notifiers |  -  |
| **401** | Unauthorized |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## testNotifier

> TestNotifierResponse testNotifier(id)

Send a test notification through a notifier. Admin only.

For Slack and Discord, sends a test message via the configured webhook URL. For generic webhooks, sends a JSON test payload. For SMTP, sends a real email via [&#x60;lettre&#x60;] using the notifier\&#39;s configured credentials.  # Errors  Returns [&#x60;ApiError::Forbidden&#x60;] for non-admins, [&#x60;ApiError::NotFound&#x60;] when the notifier does not exist, or [&#x60;ApiError::Internal&#x60;] when the notification fails with a truly internal error. Upstream failures (bad credentials, unreachable SMTP server, invalid addresses) are returned with HTTP 200 and &#x60;success: false&#x60; in the body, matching the convention established by the Slack/Discord/Webhook arms.

### Example

```ts
import {
  Configuration,
  NotifiersApi,
} from '@zlayer/client';
import type { TestNotifierRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new NotifiersApi(config);

  const body = {
    // string | Notifier id
    id: id_example,
  } satisfies TestNotifierRequest;

  try {
    const data = await api.testNotifier(body);
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
| **id** | `string` | Notifier id | [Defaults to `undefined`] |

### Return type

[**TestNotifierResponse**](TestNotifierResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Test notification sent |  -  |
| **403** | Admin role required |  -  |
| **404** | Not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## updateNotifier

> StoredNotifier updateNotifier(id, updateNotifierRequest)

Update a notifier. Admin only.

Supports partial updates: only fields present in the request body are changed.  # Errors  Returns [&#x60;ApiError::Forbidden&#x60;] for non-admins, [&#x60;ApiError::NotFound&#x60;] when the notifier does not exist, [&#x60;ApiError::BadRequest&#x60;] for validation errors, or [&#x60;ApiError::Internal&#x60;] when the store fails.

### Example

```ts
import {
  Configuration,
  NotifiersApi,
} from '@zlayer/client';
import type { UpdateNotifierOperationRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new NotifiersApi(config);

  const body = {
    // string | Notifier id
    id: id_example,
    // UpdateNotifierRequest
    updateNotifierRequest: ...,
  } satisfies UpdateNotifierOperationRequest;

  try {
    const data = await api.updateNotifier(body);
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
| **id** | `string` | Notifier id | [Defaults to `undefined`] |
| **updateNotifierRequest** | [UpdateNotifierRequest](UpdateNotifierRequest.md) |  | |

### Return type

[**StoredNotifier**](StoredNotifier.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: `application/json`
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Notifier updated |  -  |
| **400** | Invalid request |  -  |
| **403** | Admin role required |  -  |
| **404** | Not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)

