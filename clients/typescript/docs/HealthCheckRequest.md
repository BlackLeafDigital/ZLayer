
# HealthCheckRequest

Container health check request.  Mirrors the on-disk `HealthCheck` enum (see `zlayer_spec::HealthCheck`) as a discriminated union keyed on `type`. Translated to `zlayer_spec::HealthSpec` by `HealthCheckRequest::to_health_spec`. Durations are humantime strings (for example `\"10s\"`, `\"500ms\"`, `\"1m\"`).  ## Variants - `type: \"tcp\"` — requires `port` (1-65535). - `type: \"http\"` — requires `url`; `expect_status` defaults to 200. - `type: \"command\"` — requires `command` (array of argv tokens; joined with   spaces and passed to `sh -c` by the health monitor, matching the existing   compose-to-ZLayer conversion in `zlayer-docker`).

## Properties

Name | Type
------------ | -------------
`command` | Array&lt;string&gt;
`expectStatus` | number
`interval` | string
`port` | number
`retries` | number
`startPeriod` | string
`timeout` | string
`type` | string
`url` | string

## Example

```typescript
import type { HealthCheckRequest } from '@zlayer/api-client'

// TODO: Update the object below with actual values
const example = {
  "command": null,
  "expectStatus": null,
  "interval": null,
  "port": null,
  "retries": null,
  "startPeriod": null,
  "timeout": null,
  "type": null,
  "url": null,
} satisfies HealthCheckRequest

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as HealthCheckRequest
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


