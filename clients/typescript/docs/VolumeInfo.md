
# VolumeInfo

Full volume response shape used by the list, inspect, and create endpoints.

## Properties

Name | Type
------------ | -------------
`createdAt` | string
`inUseBy` | Array&lt;string&gt;
`labels` | { [key: string]: string; }
`name` | string
`path` | string
`sizeBytes` | number

## Example

```typescript
import type { VolumeInfo } from '@zlayer/api-client'

// TODO: Update the object below with actual values
const example = {
  "createdAt": null,
  "inUseBy": null,
  "labels": null,
  "name": null,
  "path": null,
  "sizeBytes": null,
} satisfies VolumeInfo

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as VolumeInfo
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


