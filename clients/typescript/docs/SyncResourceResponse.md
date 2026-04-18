
# SyncResourceResponse

A single resource in the diff output.

## Properties

Name | Type
------------ | -------------
`filePath` | string
`kind` | string
`name` | string

## Example

```typescript
import type { SyncResourceResponse } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "filePath": null,
  "kind": null,
  "name": null,
} satisfies SyncResourceResponse

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as SyncResourceResponse
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


