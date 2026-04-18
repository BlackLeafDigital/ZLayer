
# SyncDiffResponse

JSON-friendly wrapper around [`SyncDiff`].

## Properties

Name | Type
------------ | -------------
`toCreate` | [Array&lt;SyncResourceResponse&gt;](SyncResourceResponse.md)
`toDelete` | Array&lt;string&gt;
`toUpdate` | [Array&lt;SyncResourceResponse&gt;](SyncResourceResponse.md)

## Example

```typescript
import type { SyncDiffResponse } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "toCreate": null,
  "toDelete": null,
  "toUpdate": null,
} satisfies SyncDiffResponse

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as SyncDiffResponse
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


