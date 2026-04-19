
# HeartbeatRequest

Heartbeat request from a worker node.

## Properties

Name | Type
------------ | -------------
`cpuUsed` | number
`diskUsed` | number
`gpuUtilization` | [Array&lt;GpuUtilizationReport&gt;](GpuUtilizationReport.md)
`memoryUsed` | number
`nodeId` | number

## Example

```typescript
import type { HeartbeatRequest } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "cpuUsed": null,
  "diskUsed": null,
  "gpuUtilization": null,
  "memoryUsed": null,
  "nodeId": null,
} satisfies HeartbeatRequest

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as HeartbeatRequest
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


