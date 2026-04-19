# WebhooksApi

All URIs are relative to *http://localhost*

| Method | HTTP request | Description |
|------------- | ------------- | -------------|
| [**getWebhookInfo**](WebhooksApi.md#getwebhookinfo) | **GET** /api/v1/projects/{id}/webhook | Get webhook configuration for a project. |
| [**receiveWebhook**](WebhooksApi.md#receivewebhook) | **POST** /webhooks/{provider}/{project_id} | Receive a webhook push event and trigger a project pull. |
| [**rotateWebhookSecret**](WebhooksApi.md#rotatewebhooksecret) | **POST** /api/v1/projects/{id}/webhook/rotate | Rotate (regenerate) the webhook secret for a project. |



## getWebhookInfo

> WebhookInfoResponse getWebhookInfo(id)

Get webhook configuration for a project.

Returns the webhook URL pattern and the HMAC secret. Generates the secret on first call if it does not exist.  Auth required (any authenticated user).  # Errors  Returns [&#x60;ApiError::NotFound&#x60;] when the project does not exist, or [&#x60;ApiError::Internal&#x60;] on store failures.

### Example

```ts
import {
  Configuration,
  WebhooksApi,
} from '@zlayer/client';
import type { GetWebhookInfoRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new WebhooksApi(config);

  const body = {
    // string | Project id
    id: id_example,
  } satisfies GetWebhookInfoRequest;

  try {
    const data = await api.getWebhookInfo(body);
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
| **id** | `string` | Project id | [Defaults to `undefined`] |

### Return type

[**WebhookInfoResponse**](WebhookInfoResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Webhook configuration |  -  |
| **404** | Project not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## receiveWebhook

> WebhookResponse receiveWebhook(provider, projectId, body)

Receive a webhook push event and trigger a project pull.

This endpoint is **unauthenticated** -- it relies on HMAC signature verification against the per-project webhook secret.  Supported providers: &#x60;github&#x60;, &#x60;gitea&#x60;, &#x60;forgejo&#x60;, &#x60;gitlab&#x60;.  # Errors  Returns [&#x60;ApiError::NotFound&#x60;] when no webhook secret is configured, [&#x60;ApiError::Forbidden&#x60;] when signature verification fails, [&#x60;ApiError::BadRequest&#x60;] for unsupported providers or missing git URL, or [&#x60;ApiError::Internal&#x60;] on git operation failures.

### Example

```ts
import {
  Configuration,
  WebhooksApi,
} from '@zlayer/client';
import type { ReceiveWebhookRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const api = new WebhooksApi();

  const body = {
    // string | Git host provider (github, gitea, forgejo, gitlab)
    provider: provider_example,
    // string | Project id
    projectId: projectId_example,
    // string | Raw webhook payload from the git host
    body: body_example,
  } satisfies ReceiveWebhookRequest;

  try {
    const data = await api.receiveWebhook(body);
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
| **provider** | `string` | Git host provider (github, gitea, forgejo, gitlab) | [Defaults to `undefined`] |
| **projectId** | `string` | Project id | [Defaults to `undefined`] |
| **body** | `string` | Raw webhook payload from the git host | |

### Return type

[**WebhookResponse**](WebhookResponse.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: `application/json`
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Pull triggered |  -  |
| **400** | Bad request (unsupported provider or project has no git URL) |  -  |
| **403** | Signature verification failed |  -  |
| **404** | Project not found or no webhook secret configured |  -  |
| **500** | Pull failed |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## rotateWebhookSecret

> WebhookInfoResponse rotateWebhookSecret(id)

Rotate (regenerate) the webhook secret for a project.

Admin only.  # Errors  Returns [&#x60;ApiError::Forbidden&#x60;] for non-admins, [&#x60;ApiError::NotFound&#x60;] when the project does not exist, or [&#x60;ApiError::Internal&#x60;] on store failures.

### Example

```ts
import {
  Configuration,
  WebhooksApi,
} from '@zlayer/client';
import type { RotateWebhookSecretRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new WebhooksApi(config);

  const body = {
    // string | Project id
    id: id_example,
  } satisfies RotateWebhookSecretRequest;

  try {
    const data = await api.rotateWebhookSecret(body);
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
| **id** | `string` | Project id | [Defaults to `undefined`] |

### Return type

[**WebhookInfoResponse**](WebhookInfoResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | New webhook configuration |  -  |
| **403** | Admin role required |  -  |
| **404** | Project not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)

