
# ClusterPeer

Summary of an existing cluster peer returned in join response.

## Properties

Name | Type
------------ | -------------
`advertiseAddr` | string
`nodeId` | string
`overlayIp` | string
`overlayPort` | number
`raftNodeId` | number
`raftPort` | number
`wgPublicKey` | string

## Example

```typescript
import type { ClusterPeer } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "advertiseAddr": null,
  "nodeId": null,
  "overlayIp": null,
  "overlayPort": null,
  "raftNodeId": null,
  "raftPort": null,
  "wgPublicKey": null,
} satisfies ClusterPeer

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as ClusterPeer
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


