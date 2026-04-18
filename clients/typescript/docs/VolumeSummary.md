
# VolumeSummary

Summary of a single volume returned by the list endpoint.

## Properties

Name | Type
------------ | -------------
`name` | string
`path` | string
`sizeBytes` | number

## Example

```typescript
import type { VolumeSummary } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "name": null,
  "path": null,
  "sizeBytes": null,
} satisfies VolumeSummary

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as VolumeSummary
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


