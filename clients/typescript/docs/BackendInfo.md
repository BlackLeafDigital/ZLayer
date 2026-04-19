
# BackendInfo

Information about a single backend in a load-balancer group.

## Properties

Name | Type
------------ | -------------
`activeConnections` | number
`address` | string
`consecutiveFailures` | number
`healthy` | boolean

## Example

```typescript
import type { BackendInfo } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "activeConnections": null,
  "address": null,
  "consecutiveFailures": null,
  "healthy": null,
} satisfies BackendInfo

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as BackendInfo
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


