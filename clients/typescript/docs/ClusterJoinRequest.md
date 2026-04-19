
# ClusterJoinRequest

Request body for `POST /api/v1/cluster/join`.

## Properties

Name | Type
------------ | -------------
`advertiseAddr` | string
`apiPort` | number
`cpuTotal` | number
`diskTotal` | number
`gpus` | [Array&lt;GpuInfoSummary&gt;](GpuInfoSummary.md)
`memoryTotal` | number
`mode` | string
`overlayPort` | number
`raftPort` | number
`services` | Array&lt;string&gt;
`token` | string
`wgPublicKey` | string

## Example

```typescript
import type { ClusterJoinRequest } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "advertiseAddr": null,
  "apiPort": null,
  "cpuTotal": null,
  "diskTotal": null,
  "gpus": null,
  "memoryTotal": null,
  "mode": null,
  "overlayPort": null,
  "raftPort": null,
  "services": null,
  "token": null,
  "wgPublicKey": null,
} satisfies ClusterJoinRequest

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as ClusterJoinRequest
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


