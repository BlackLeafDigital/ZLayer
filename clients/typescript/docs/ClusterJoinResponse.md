
# ClusterJoinResponse

Response body for `POST /api/v1/cluster/join`.

## Properties

Name | Type
------------ | -------------
`nodeId` | string
`overlayIp` | string
`peers` | [Array&lt;ClusterPeer&gt;](ClusterPeer.md)
`raftNodeId` | number
`role` | string

## Example

```typescript
import type { ClusterJoinResponse } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "nodeId": null,
  "overlayIp": null,
  "peers": null,
  "raftNodeId": null,
  "role": null,
} satisfies ClusterJoinResponse

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as ClusterJoinResponse
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


