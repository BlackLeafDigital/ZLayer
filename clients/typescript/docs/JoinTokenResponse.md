
# JoinTokenResponse

Join token response for cluster joining

## Properties

Name | Type
------------ | -------------
`allocatedIp` | string
`expiresAt` | number
`leaderEndpoint` | string
`leaderPublicKey` | string
`overlayCidr` | string
`token` | string

## Example

```typescript
import type { JoinTokenResponse } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "allocatedIp": null,
  "expiresAt": null,
  "leaderEndpoint": null,
  "leaderPublicKey": null,
  "overlayCidr": null,
  "token": null,
} satisfies JoinTokenResponse

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as JoinTokenResponse
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


