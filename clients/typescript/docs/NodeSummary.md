
# NodeSummary

Node summary for list operations

## Properties

Name | Type
------------ | -------------
`address` | string
`id` | number
`labels` | { [key: string]: string; }
`lastSeen` | number
`role` | string
`status` | string

## Example

```typescript
import type { NodeSummary } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "address": null,
  "id": null,
  "labels": null,
  "lastSeen": null,
  "role": null,
  "status": null,
} satisfies NodeSummary

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as NodeSummary
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


