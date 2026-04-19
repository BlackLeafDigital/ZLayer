
# ClusterNodeSummary

Summary of a cluster node for listing.

## Properties

Name | Type
------------ | -------------
`address` | string
`advertiseAddr` | string
`cpuTotal` | number
`cpuUsed` | number
`id` | string
`isLeader` | boolean
`lastHeartbeat` | number
`memoryTotal` | number
`memoryUsed` | number
`mode` | string
`overlayIp` | string
`registeredAt` | number
`role` | string
`status` | string

## Example

```typescript
import type { ClusterNodeSummary } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "address": null,
  "advertiseAddr": null,
  "cpuTotal": null,
  "cpuUsed": null,
  "id": null,
  "isLeader": null,
  "lastHeartbeat": null,
  "memoryTotal": null,
  "memoryUsed": null,
  "mode": null,
  "overlayIp": null,
  "registeredAt": null,
  "role": null,
  "status": null,
} satisfies ClusterNodeSummary

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as ClusterNodeSummary
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


