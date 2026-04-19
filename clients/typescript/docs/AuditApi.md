# AuditApi

All URIs are relative to *http://localhost*

| Method | HTTP request | Description |
|------------- | ------------- | -------------|
| [**listAudit**](AuditApi.md#listaudit) | **GET** /api/v1/audit | List audit log entries. Admin only. |



## listAudit

> Array&lt;AuditEntry&gt; listAudit(user, resourceKind, since, until, limit)

List audit log entries. Admin only.

# Errors  Returns [&#x60;ApiError::Forbidden&#x60;] when the caller is not an admin, or [&#x60;ApiError::Internal&#x60;] if the store fails.

### Example

```ts
import {
  Configuration,
  AuditApi,
} from '@zlayer/client';
import type { ListAuditRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const api = new AuditApi();

  const body = {
    // string | Filter by user id. (optional)
    user: user_example,
    // string | Filter by resource kind. (optional)
    resourceKind: resourceKind_example,
    // string | Only entries at or after this timestamp (RFC 3339). (optional)
    since: since_example,
    // string | Only entries at or before this timestamp (RFC 3339). (optional)
    until: until_example,
    // number | Maximum number of entries to return (default 100). (optional)
    limit: 56,
  } satisfies ListAuditRequest;

  try {
    const data = await api.listAudit(body);
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
| **user** | `string` | Filter by user id. | [Optional] [Defaults to `undefined`] |
| **resourceKind** | `string` | Filter by resource kind. | [Optional] [Defaults to `undefined`] |
| **since** | `string` | Only entries at or after this timestamp (RFC 3339). | [Optional] [Defaults to `undefined`] |
| **until** | `string` | Only entries at or before this timestamp (RFC 3339). | [Optional] [Defaults to `undefined`] |
| **limit** | `number` | Maximum number of entries to return (default 100). | [Optional] [Defaults to `undefined`] |

### Return type

[**Array&lt;AuditEntry&gt;**](AuditEntry.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | List audit entries |  -  |
| **403** | Admin role required |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)

