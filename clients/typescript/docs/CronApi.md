# CronApi

All URIs are relative to *http://localhost*

| Method | HTTP request | Description |
|------------- | ------------- | -------------|
| [**disableCronJob**](CronApi.md#disablecronjob) | **PUT** /api/v1/cron/{name}/disable | PUT /api/v1/cron/{name}/disable - Disable a cron job |
| [**enableCronJob**](CronApi.md#enablecronjob) | **PUT** /api/v1/cron/{name}/enable | PUT /api/v1/cron/{name}/enable - Enable a cron job |
| [**getCronJob**](CronApi.md#getcronjob) | **GET** /api/v1/cron/{name} | GET /api/v1/cron/{name} - Get cron job details |
| [**listCronJobs**](CronApi.md#listcronjobs) | **GET** /api/v1/cron | GET /api/v1/cron - List all cron jobs |
| [**triggerCronJob**](CronApi.md#triggercronjob) | **POST** /api/v1/cron/{name}/trigger | POST /api/v1/cron/{name}/trigger - Manually trigger a cron job |



## disableCronJob

> CronStatusResponse disableCronJob(name)

PUT /api/v1/cron/{name}/disable - Disable a cron job

Disables a cron job, preventing it from running on schedule. The job can still be manually triggered.  # Errors  Returns an error if the cron job is not found or the user lacks permission.

### Example

```ts
import {
  Configuration,
  CronApi,
} from '@zlayer/client';
import type { DisableCronJobRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new CronApi(config);

  const body = {
    // string | Cron job name
    name: name_example,
  } satisfies DisableCronJobRequest;

  try {
    const data = await api.disableCronJob(body);
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
| **name** | `string` | Cron job name | [Defaults to `undefined`] |

### Return type

[**CronStatusResponse**](CronStatusResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Cron job disabled |  -  |
| **401** | Unauthorized |  -  |
| **404** | Cron job not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## enableCronJob

> CronStatusResponse enableCronJob(name)

PUT /api/v1/cron/{name}/enable - Enable a cron job

Enables a disabled cron job, allowing it to run on schedule.  # Errors  Returns an error if the cron job is not found or the user lacks permission.

### Example

```ts
import {
  Configuration,
  CronApi,
} from '@zlayer/client';
import type { EnableCronJobRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new CronApi(config);

  const body = {
    // string | Cron job name
    name: name_example,
  } satisfies EnableCronJobRequest;

  try {
    const data = await api.enableCronJob(body);
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
| **name** | `string` | Cron job name | [Defaults to `undefined`] |

### Return type

[**CronStatusResponse**](CronStatusResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Cron job enabled |  -  |
| **401** | Unauthorized |  -  |
| **404** | Cron job not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## getCronJob

> CronJobResponse getCronJob(name)

GET /api/v1/cron/{name} - Get cron job details

Returns detailed information about a specific cron job.  # Errors  Returns an error if the cron job is not found.

### Example

```ts
import {
  Configuration,
  CronApi,
} from '@zlayer/client';
import type { GetCronJobRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new CronApi(config);

  const body = {
    // string | Cron job name
    name: name_example,
  } satisfies GetCronJobRequest;

  try {
    const data = await api.getCronJob(body);
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
| **name** | `string` | Cron job name | [Defaults to `undefined`] |

### Return type

[**CronJobResponse**](CronJobResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | Cron job details |  -  |
| **401** | Unauthorized |  -  |
| **404** | Cron job not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## listCronJobs

> Array&lt;CronJobResponse&gt; listCronJobs()

GET /api/v1/cron - List all cron jobs

Returns a list of all registered cron jobs with their schedule information.  # Errors  Returns an error if the user is not authenticated.

### Example

```ts
import {
  Configuration,
  CronApi,
} from '@zlayer/client';
import type { ListCronJobsRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new CronApi(config);

  try {
    const data = await api.listCronJobs();
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

[**Array&lt;CronJobResponse&gt;**](CronJobResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **200** | List of cron jobs |  -  |
| **401** | Unauthorized |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


## triggerCronJob

> TriggerCronResponse triggerCronJob(name)

POST /api/v1/cron/{name}/trigger - Manually trigger a cron job

Triggers an immediate execution of the cron job, regardless of its schedule.  # Errors  Returns an error if the cron job is not found or triggering fails.

### Example

```ts
import {
  Configuration,
  CronApi,
} from '@zlayer/client';
import type { TriggerCronJobRequest } from '@zlayer/client';

async function example() {
  console.log("🚀 Testing @zlayer/client SDK...");
  const config = new Configuration({ 
    // Configure HTTP bearer authorization: bearer_auth
    accessToken: "YOUR BEARER TOKEN",
  });
  const api = new CronApi(config);

  const body = {
    // string | Cron job name
    name: name_example,
  } satisfies TriggerCronJobRequest;

  try {
    const data = await api.triggerCronJob(body);
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
| **name** | `string` | Cron job name | [Defaults to `undefined`] |

### Return type

[**TriggerCronResponse**](TriggerCronResponse.md)

### Authorization

[bearer_auth](../README.md#bearer_auth)

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: `application/json`


### HTTP response details
| Status code | Description | Response headers |
|-------------|-------------|------------------|
| **202** | Cron job triggered |  -  |
| **401** | Unauthorized |  -  |
| **404** | Cron job not found |  -  |

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)

