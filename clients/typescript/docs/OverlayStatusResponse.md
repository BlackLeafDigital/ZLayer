
# OverlayStatusResponse

Overlay network status response

## Properties

Name | Type
------------ | -------------
`cidr` | string
`healthyPeers` | number
`_interface` | string
`isLeader` | boolean
`lastCheck` | number
`nodeIp` | string
`port` | number
`totalPeers` | number
`unhealthyPeers` | number

## Example

```typescript
import type { OverlayStatusResponse } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "cidr": null,
  "healthyPeers": null,
  "_interface": null,
  "isLeader": null,
  "lastCheck": null,
  "nodeIp": null,
  "port": null,
  "totalPeers": null,
  "unhealthyPeers": null,
} satisfies OverlayStatusResponse

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as OverlayStatusResponse
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


