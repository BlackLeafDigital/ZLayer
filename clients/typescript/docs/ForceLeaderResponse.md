
# ForceLeaderResponse

Response body for force-leader operation.

## Properties

Name | Type
------------ | -------------
`message` | string
`preservedNodes` | number
`preservedServices` | number
`success` | boolean

## Example

```typescript
import type { ForceLeaderResponse } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "message": null,
  "preservedNodes": null,
  "preservedServices": null,
  "success": null,
} satisfies ForceLeaderResponse

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as ForceLeaderResponse
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


