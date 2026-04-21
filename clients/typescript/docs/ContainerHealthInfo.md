
# ContainerHealthInfo

Runtime-native health snapshot on [`ContainerInfo::health`].  Sourced from bollard\'s `ContainerState.health` for Docker-backed containers. The internal `HealthMonitor` in `crates/zlayer-agent/src/health.rs` drives service-level health events against user-configured health specs; for standalone containers the API reports the runtime-native status instead so images with a baked-in `HEALTHCHECK` still surface correctly.

## Properties

Name | Type
------------ | -------------
`failingStreak` | number
`lastOutput` | string
`status` | string

## Example

```typescript
import type { ContainerHealthInfo } from '@zlayer/api-client'

// TODO: Update the object below with actual values
const example = {
  "failingStreak": null,
  "lastOutput": null,
  "status": null,
} satisfies ContainerHealthInfo

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as ContainerHealthInfo
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


