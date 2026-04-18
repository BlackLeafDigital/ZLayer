
# PeerInfo

Peer information

## Properties

Name | Type
------------ | -------------
`failureCount` | number
`healthy` | boolean
`lastCheck` | number
`lastHandshakeSecs` | number
`lastPingMs` | number
`overlayIp` | string
`publicKey` | string

## Example

```typescript
import type { PeerInfo } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "failureCount": null,
  "healthy": null,
  "lastCheck": null,
  "lastHandshakeSecs": null,
  "lastPingMs": null,
  "overlayIp": null,
  "publicKey": null,
} satisfies PeerInfo

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as PeerInfo
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


